//! Bounded authenticated B+ pages. Edits visit only intersecting branches;
//! bootstrap consumes sorted rows without retaining a corpus-sized map.

use std::sync::Arc;

use quanta_index_core::{CoreError, RequestBudgetV1};

use super::codec::{BlobRef, MapEdit, MapRoot, ObjectKind, ObjectRef, PageRef};
use super::store::{AttemptLease, ObjectStore, Page, corrupt, invalid, storage};

const PAGE_MAGIC: &[u8; 8] = b"QLMAP002";
const LEAF_INLINE: u8 = 0;
const LEAF_DICTIONARY: u8 = 1;
const BLOB_REF_WIRE_BYTES: usize = 81;
const MAX_KEY_BYTES: usize = 256;
const LEAF_FANOUT: usize = 96;
const BRANCH_FANOUT: usize = 64;
pub(crate) const MAX_MAP_HEIGHT: u8 = 12;

struct EditCursor<I> {
    source: I,
    next: Option<MapEdit>,
    previous: Option<Vec<u8>>,
    done: bool,
}

impl<I: Iterator<Item = Result<MapEdit, CoreError>>> EditCursor<I> {
    fn new(source: I) -> Self {
        Self {
            source,
            next: None,
            previous: None,
            done: false,
        }
    }

    fn peek(&mut self) -> Result<Option<&MapEdit>, CoreError> {
        if self.next.is_none() && !self.done {
            match self.source.next() {
                Some(Ok(edit)) => {
                    check_key(&edit.key)?;
                    if self
                        .previous
                        .as_ref()
                        .is_some_and(|key| key.as_slice() >= edit.key.as_slice())
                    {
                        return Err(invalid("map edits are not strictly sorted"));
                    }
                    self.previous = Some(edit.key.clone());
                    self.next = Some(edit);
                }
                Some(Err(error)) => return Err(error),
                None => self.done = true,
            }
        }
        Ok(self.next.as_ref())
    }

    fn take(&mut self) -> Result<MapEdit, CoreError> {
        let _peeked: Option<&MapEdit> = self.peek()?;
        self.next
            .take()
            .ok_or_else(|| invalid("map edit cursor is exhausted"))
    }

    fn has_before(&mut self, upper: Option<&[u8]>) -> Result<bool, CoreError> {
        Ok(self
            .peek()?
            .is_some_and(|edit| upper.is_none_or(|upper| edit.key.as_slice() <= upper)))
    }
}

type PageExtent = (Vec<u8>, Vec<u8>, u64, u8, Vec<ObjectRef>);
type EncodedPage = (Vec<u8>, Vec<ObjectRef>, Vec<u8>, Vec<u8>, u64, u8);
type ChangeVisitor<'a> =
    dyn FnMut(&[u8], Option<&BlobRef>, Option<&BlobRef>) -> Result<(), CoreError> + 'a;
type ScanVisitor<'a> = dyn FnMut(&[u8], &BlobRef) -> Result<(), CoreError> + 'a;

enum DiffItem {
    Page(PageRef),
    Row(Vec<u8>, BlobRef),
}

impl DiffItem {
    fn first(&self) -> &[u8] {
        match self {
            Self::Page(page) => &page.first,
            Self::Row(key, _) => key,
        }
    }

    fn last(&self) -> &[u8] {
        match self {
            Self::Page(page) => &page.last,
            Self::Row(key, _) => key,
        }
    }
}

struct CursorFrame {
    page: Page,
    next: usize,
}

/// Ordered, fallible map walk retaining only one page per level.
pub(crate) struct MapCursor<'a> {
    store: &'a ObjectStore,
    pending: Option<PageRef>,
    frames: Vec<CursorFrame>,
    lower: Vec<u8>,
    upper: Option<Vec<u8>>,
    budget: Option<&'a RequestBudgetV1>,
}

impl MapCursor<'_> {
    pub(crate) fn next(&mut self) -> Result<Option<(Vec<u8>, BlobRef)>, CoreError> {
        loop {
            if let Some(reference) = self.pending.take() {
                if self.frames.len() >= usize::from(MAX_MAP_HEIGHT).saturating_add(1) {
                    return Err(corrupt("map cursor depth exceeds bound"));
                }
                let page = self.store.load_page(&reference, self.budget)?;
                self.frames
                    .try_reserve(1)
                    .map_err(|error| storage("reserve map cursor frame", error))?;
                self.frames.push(CursorFrame { page, next: 0 });
            }
            let Some(frame) = self.frames.last_mut() else {
                return Ok(None);
            };
            match &frame.page {
                Page::Leaf(rows) => {
                    let Some((key, value)) = rows.get(frame.next) else {
                        drop(self.frames.pop());
                        continue;
                    };
                    frame.next = frame
                        .next
                        .checked_add(1)
                        .ok_or_else(|| corrupt("map cursor leaf offset overflow"))?;
                    if key.as_slice() < self.lower.as_slice() {
                        continue;
                    }
                    if self
                        .upper
                        .as_ref()
                        .is_some_and(|upper| key.as_slice() >= upper.as_slice())
                    {
                        self.frames.clear();
                        return Ok(None);
                    }
                    return Ok(Some((key.clone(), value.clone())));
                }
                Page::Branch(children) => {
                    let Some(child) = children.get(frame.next) else {
                        drop(self.frames.pop());
                        continue;
                    };
                    frame.next = frame
                        .next
                        .checked_add(1)
                        .ok_or_else(|| corrupt("map cursor branch offset overflow"))?;
                    if child.last.as_slice() < self.lower.as_slice() {
                        continue;
                    }
                    if self
                        .upper
                        .as_ref()
                        .is_some_and(|upper| child.first.as_slice() >= upper.as_slice())
                    {
                        self.frames.clear();
                        return Ok(None);
                    }
                    self.pending = Some(child.clone());
                }
            }
        }
    }
}

fn checkpoint(budget: Option<&RequestBudgetV1>) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint("lexical:map:traverse")?;
    }
    Ok(())
}

fn check_key(key: &[u8]) -> Result<(), CoreError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(invalid("map key exceeds physical page bound"));
    }
    Ok(())
}

fn u16_len(len: usize) -> Result<u16, CoreError> {
    u16::try_from(len).map_err(|_conversion_error| invalid("page field length exceeds u16"))
}

fn add_key(buffer: &mut Vec<u8>, key: &[u8]) -> Result<(), CoreError> {
    check_key(key)?;
    buffer.extend_from_slice(&u16_len(key.len())?.to_be_bytes());
    buffer.extend_from_slice(key);
    Ok(())
}

fn add_object_ref(buffer: &mut Vec<u8>, reference: &ObjectRef) {
    buffer.push(reference.kind.code());
    buffer.extend_from_slice(&reference.bytes.to_be_bytes());
    buffer.extend_from_slice(&reference.sha256);
}

fn add_blob_ref(buffer: &mut Vec<u8>, value: &BlobRef) {
    add_object_ref(buffer, &value.root);
    buffer.extend_from_slice(&value.bytes.to_be_bytes());
    buffer.extend_from_slice(&value.sha256);
}

fn distinct_leaf_values(rows: &[(Vec<u8>, BlobRef)]) -> Result<Vec<BlobRef>, CoreError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(rows.len())
        .map_err(|error| storage("reserve leaf value dictionary", error))?;
    values.extend(rows.iter().map(|(_, value)| value.clone()));
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn dictionary_is_smaller(rows: usize, unique_values: usize) -> bool {
    // Both V2 modes carry the one-byte tag. Fanout bounds make these
    // saturating operations exact; equal-sized encodings use inline mode.
    2_usize
        .saturating_add(BLOB_REF_WIRE_BYTES.saturating_mul(unique_values))
        .saturating_add(2_usize.saturating_mul(rows))
        < BLOB_REF_WIRE_BYTES.saturating_mul(rows)
}

fn page_extent(page: &Page) -> Result<PageExtent, CoreError> {
    match page {
        Page::Leaf(entries) => {
            if entries.is_empty() || entries.len() > LEAF_FANOUT {
                return Err(invalid("leaf page fanout"));
            }
            let first = entries
                .first()
                .ok_or_else(|| invalid("empty leaf"))?
                .0
                .clone();
            let last = entries
                .last()
                .ok_or_else(|| invalid("empty leaf"))?
                .0
                .clone();
            let count =
                u64::try_from(entries.len()).map_err(|error| storage("leaf count width", error))?;
            let mut dependencies = Vec::new();
            dependencies
                .try_reserve_exact(entries.len())
                .map_err(|error| storage("reserve leaf dependencies", error))?;
            dependencies.extend(entries.iter().map(|(_, blob)| blob.root.clone()));
            Ok((first, last, count, 0, dependencies))
        }
        Page::Branch(children) => {
            if children.is_empty() || children.len() > BRANCH_FANOUT {
                return Err(invalid("branch page fanout"));
            }
            let first = children
                .first()
                .ok_or_else(|| invalid("empty branch"))?
                .first
                .clone();
            let last = children
                .last()
                .ok_or_else(|| invalid("empty branch"))?
                .last
                .clone();
            let height = children
                .first()
                .ok_or_else(|| invalid("empty branch"))?
                .height
                .checked_add(1)
                .ok_or_else(|| invalid("map height overflow"))?;
            if height > MAX_MAP_HEIGHT {
                return Err(invalid("map height exceeds bound"));
            }
            let count = children
                .iter()
                .try_fold(0_u64, |sum, child| sum.checked_add(child.entries))
                .ok_or_else(|| invalid("map subtree count overflow"))?;
            let mut dependencies = Vec::new();
            dependencies
                .try_reserve_exact(children.len())
                .map_err(|error| storage("reserve branch dependencies", error))?;
            dependencies.extend(children.iter().map(|child| child.object.clone()));
            Ok((first, last, count, height, dependencies))
        }
    }
}

fn encode_page(page: &Page) -> Result<EncodedPage, CoreError> {
    let (first, last, entries, height, dependencies) = page_extent(page)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(48 * 1024)
        .map_err(|error| storage("reserve map page", error))?;
    bytes.extend_from_slice(PAGE_MAGIC);
    bytes.push(height);
    let count = match page {
        Page::Leaf(rows) => rows.len(),
        Page::Branch(children) => children.len(),
    };
    bytes.extend_from_slice(&u16_len(count)?.to_be_bytes());
    bytes.extend_from_slice(&entries.to_be_bytes());
    add_key(&mut bytes, &first)?;
    add_key(&mut bytes, &last)?;
    match page {
        Page::Leaf(rows) => {
            let dictionary = distinct_leaf_values(rows)?;
            if dictionary_is_smaller(rows.len(), dictionary.len()) {
                bytes.push(LEAF_DICTIONARY);
                bytes.extend_from_slice(&u16_len(dictionary.len())?.to_be_bytes());
                for value in &dictionary {
                    add_blob_ref(&mut bytes, value);
                }
                for (key, value) in rows {
                    add_key(&mut bytes, key)?;
                    let index = dictionary
                        .binary_search(value)
                        .map_err(|_index| invalid("leaf dictionary omits a value"))?;
                    bytes.extend_from_slice(&u16_len(index)?.to_be_bytes());
                }
            } else {
                bytes.push(LEAF_INLINE);
                for (key, value) in rows {
                    add_key(&mut bytes, key)?;
                    add_blob_ref(&mut bytes, value);
                }
            }
        }
        Page::Branch(children) => {
            for child in children {
                add_object_ref(&mut bytes, &child.object);
                bytes.extend_from_slice(&child.entries.to_be_bytes());
                bytes.push(child.height);
                add_key(&mut bytes, &child.first)?;
                add_key(&mut bytes, &child.last)?;
            }
        }
    }
    if bytes.len() > 64 * 1024 {
        return Err(invalid("encoded map page exceeds physical bound"));
    }
    Ok((bytes, dependencies, first, last, entries, height))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], CoreError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| corrupt("map cursor overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| corrupt("truncated map page"))?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, CoreError> {
        Ok(*self
            .take(1)?
            .first()
            .ok_or_else(|| corrupt("map u8 absent"))?)
    }
    fn u16(&mut self) -> Result<u16, CoreError> {
        let raw = self.take(2)?;
        Ok(u16::from_be_bytes(
            raw.try_into()
                .map_err(|_conversion_error| corrupt("map u16 width"))?,
        ))
    }
    fn u64(&mut self) -> Result<u64, CoreError> {
        let mut raw = [0_u8; 8];
        raw.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(raw))
    }
    fn digest(&mut self) -> Result<[u8; 32], CoreError> {
        let mut raw = [0_u8; 32];
        raw.copy_from_slice(self.take(32)?);
        Ok(raw)
    }
    fn key(&mut self) -> Result<Vec<u8>, CoreError> {
        let len = usize::from(self.u16()?);
        if len > MAX_KEY_BYTES {
            return Err(corrupt("map key exceeds bound"));
        }
        Ok(self.take(len)?.to_vec())
    }
    fn object_ref(&mut self) -> Result<ObjectRef, CoreError> {
        let kind =
            ObjectKind::from_code(self.u8()?).ok_or_else(|| corrupt("unknown map object kind"))?;
        let bytes = self.u64()?;
        let sha256 = self.digest()?;
        Ok(ObjectRef {
            kind,
            bytes,
            sha256,
        })
    }
    fn blob_ref(&mut self) -> Result<BlobRef, CoreError> {
        Ok(BlobRef {
            root: self.object_ref()?,
            bytes: self.u64()?,
            sha256: self.digest()?,
        })
    }
}

fn decode_page(
    payload: &[u8],
    reference: &PageRef,
    object_deps: &[ObjectRef],
) -> Result<Page, CoreError> {
    let mut cursor = Cursor {
        bytes: payload,
        offset: 0,
    };
    if cursor.take(8)? != PAGE_MAGIC {
        return Err(corrupt("map page magic differs"));
    }
    let height = cursor.u8()?;
    let count = usize::from(cursor.u16()?);
    let entries = cursor.u64()?;
    let first = cursor.key()?;
    let last = cursor.key()?;
    if height != reference.height
        || entries != reference.entries
        || first != reference.first
        || last != reference.last
        || height > MAX_MAP_HEIGHT
        || count == 0
    {
        return Err(corrupt("map page descriptor differs"));
    }
    let page = if height == 0 {
        if count > LEAF_FANOUT
            || entries
                != u64::try_from(count).map_err(|_conversion_error| corrupt("leaf count width"))?
        {
            return Err(corrupt("leaf count differs"));
        }
        let mode = cursor.u8()?;
        let mut dictionary = Vec::new();
        let mut used = Vec::new();
        match mode {
            LEAF_INLINE => {}
            LEAF_DICTIONARY => {
                let unique_values = usize::from(cursor.u16()?);
                if unique_values == 0 || unique_values > count {
                    return Err(corrupt("leaf dictionary count differs"));
                }
                if !dictionary_is_smaller(count, unique_values) {
                    return Err(corrupt("leaf dictionary mode is not minimal"));
                }
                dictionary
                    .try_reserve_exact(unique_values)
                    .map_err(|error| storage("reserve decoded leaf dictionary", error))?;
                used.try_reserve_exact(unique_values)
                    .map_err(|error| storage("reserve leaf dictionary usage", error))?;
                for _ in 0..unique_values {
                    let value = cursor.blob_ref()?;
                    if dictionary.last().is_some_and(|prior| prior >= &value) {
                        return Err(corrupt("leaf dictionary values repeat or are not sorted"));
                    }
                    dictionary.push(value);
                    used.push(false);
                }
            }
            _ => return Err(corrupt("leaf value mode differs")),
        }
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|error| storage("reserve map leaf", error))?;
        for _ in 0..count {
            let key = cursor.key()?;
            let value = if mode == LEAF_INLINE {
                cursor.blob_ref()?
            } else {
                let index = usize::from(cursor.u16()?);
                let used_entry = used
                    .get_mut(index)
                    .ok_or_else(|| corrupt("leaf dictionary index is out of range"))?;
                *used_entry = true;
                dictionary
                    .get(index)
                    .ok_or_else(|| corrupt("leaf dictionary value is absent"))?
                    .clone()
            };
            rows.push((key, value));
        }
        if mode == LEAF_DICTIONARY && used.contains(&false) {
            return Err(corrupt("leaf dictionary contains an unused value"));
        }
        if mode == LEAF_INLINE && dictionary_is_smaller(count, distinct_leaf_values(&rows)?.len()) {
            return Err(corrupt("leaf inline mode is not minimal"));
        }
        if rows
            .windows(2)
            .any(|pair| match (pair.first(), pair.get(1)) {
                (Some(a), Some(b)) => a.0 >= b.0,
                _ => true,
            })
        {
            return Err(corrupt("leaf keys are not strictly sorted"));
        }
        Page::Leaf(rows)
    } else {
        if count > BRANCH_FANOUT {
            return Err(corrupt("branch fanout exceeds bound"));
        }
        let mut children = Vec::new();
        children
            .try_reserve_exact(count)
            .map_err(|error| storage("reserve map branch", error))?;
        for _ in 0..count {
            children.push(PageRef {
                object: cursor.object_ref()?,
                entries: cursor.u64()?,
                height: cursor.u8()?,
                first: cursor.key()?,
                last: cursor.key()?,
            });
        }
        if children.iter().any(|child| {
            child.height.checked_add(1) != Some(height)
                || child.entries == 0
                || child.first > child.last
                || child.object.kind != ObjectKind::MapPage
        }) || children
            .windows(2)
            .any(|pair| match (pair.first(), pair.get(1)) {
                (Some(a), Some(b)) => a.last >= b.first,
                _ => true,
            })
            || children
                .iter()
                .try_fold(0_u64, |sum, child| sum.checked_add(child.entries))
                != Some(entries)
        {
            return Err(corrupt("branch child ordering, height or count differs"));
        }
        Page::Branch(children)
    };
    if cursor.offset != payload.len() {
        return Err(corrupt("map page has trailing bytes"));
    }
    let (derived_first, derived_last, derived_entries, derived_height, mut dependencies) =
        page_extent(&page).map_err(|_conversion_error| corrupt("map page extent is invalid"))?;
    dependencies.sort();
    dependencies.dedup();
    if derived_first != first
        || derived_last != last
        || derived_entries != entries
        || derived_height != height
        || dependencies != object_deps
    {
        return Err(corrupt("map page fences or dependency inventory differ"));
    }
    Ok(page)
}

impl ObjectStore {
    /// Reports observable key/value changes while skipping identical authenticated
    /// subtrees even when neighboring page fences or branch heights change.
    pub(crate) fn map_changed_keys(
        &self,
        base: &MapRoot,
        new: &MapRoot,
        budget: Option<&RequestBudgetV1>,
        visitor: &mut ChangeVisitor<'_>,
    ) -> Result<(), CoreError> {
        Self::validate_root(base)?;
        Self::validate_root(new)?;
        let mut old = base
            .page
            .iter()
            .cloned()
            .map(DiffItem::Page)
            .collect::<Vec<_>>();
        let mut current = new
            .page
            .iter()
            .cloned()
            .map(DiffItem::Page)
            .collect::<Vec<_>>();
        while !old.is_empty() || !current.is_empty() {
            checkpoint(budget)?;
            let (Some(before), Some(after)) = (old.last(), current.last()) else {
                if current.is_empty() {
                    self.emit_diff_item(&mut old, true, budget, visitor)?;
                } else {
                    self.emit_diff_item(&mut current, false, budget, visitor)?;
                }
                continue;
            };
            if let (DiffItem::Page(before), DiffItem::Page(after)) = (before, after)
                && before == after
            {
                let _authenticated = self.load_page(before, budget)?;
                drop(old.pop());
                drop(current.pop());
                continue;
            }
            if before.last() < after.first() {
                self.emit_diff_item(&mut old, true, budget, visitor)?;
            } else if after.last() < before.first() {
                self.emit_diff_item(&mut current, false, budget, visitor)?;
            } else {
                match (before, after) {
                    (DiffItem::Page(left), DiffItem::Page(right))
                        if left.height >= right.height =>
                    {
                        self.expand_diff_page(&mut old, budget)?;
                    }
                    (DiffItem::Page(_) | DiffItem::Row(_, _), DiffItem::Page(_)) => {
                        self.expand_diff_page(&mut current, budget)?;
                    }
                    (DiffItem::Page(_), DiffItem::Row(_, _)) => {
                        self.expand_diff_page(&mut old, budget)?;
                    }
                    (DiffItem::Row(_, _), DiffItem::Row(_, _)) => {
                        let DiffItem::Row(key, previous) = old
                            .pop()
                            .ok_or_else(|| corrupt("old diff frontier vanished"))?
                        else {
                            return Err(corrupt("old diff row became a page"));
                        };
                        let DiffItem::Row(_, next) = current
                            .pop()
                            .ok_or_else(|| corrupt("new diff frontier vanished"))?
                        else {
                            return Err(corrupt("new diff row became a page"));
                        };
                        if previous != next {
                            visitor(&key, Some(&previous), Some(&next))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn expand_diff_page(
        &self,
        frontier: &mut Vec<DiffItem>,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let DiffItem::Page(reference) = frontier
            .pop()
            .ok_or_else(|| corrupt("diff frontier page vanished"))?
        else {
            return Err(corrupt("diff frontier row cannot expand"));
        };
        match self.load_page(&reference, budget)? {
            Page::Leaf(rows) => {
                frontier
                    .try_reserve(rows.len())
                    .map_err(|error| storage("reserve diff leaf frontier", error))?;
                frontier.extend(
                    rows.into_iter()
                        .rev()
                        .map(|(key, value)| DiffItem::Row(key, value)),
                );
            }
            Page::Branch(children) => {
                frontier
                    .try_reserve(children.len())
                    .map_err(|error| storage("reserve diff branch frontier", error))?;
                frontier.extend(children.into_iter().rev().map(DiffItem::Page));
            }
        }
        Ok(())
    }

    fn emit_diff_item(
        &self,
        frontier: &mut Vec<DiffItem>,
        from_old: bool,
        budget: Option<&RequestBudgetV1>,
        visitor: &mut ChangeVisitor<'_>,
    ) -> Result<(), CoreError> {
        match frontier
            .pop()
            .ok_or_else(|| corrupt("diff frontier item vanished"))?
        {
            DiffItem::Row(key, value) if from_old => visitor(&key, Some(&value), None),
            DiffItem::Row(key, value) => visitor(&key, None, Some(&value)),
            DiffItem::Page(page) if from_old => {
                self.scan_page(&page, b"", None, budget, &mut |key, value| {
                    visitor(key, Some(value), None)
                })
            }
            DiffItem::Page(page) => self.scan_page(&page, b"", None, budget, &mut |key, value| {
                visitor(key, None, Some(value))
            }),
        }
    }

    pub(crate) fn map_cursor<'a>(
        &'a self,
        root: &MapRoot,
        lower: &[u8],
        upper: Option<&[u8]>,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<MapCursor<'a>, CoreError> {
        check_key(lower)?;
        if let Some(upper) = upper {
            check_key(upper)?;
            if upper <= lower {
                return Err(invalid("map cursor upper bound is not above lower"));
            }
        }
        Self::validate_root(root)?;
        Ok(MapCursor {
            store: self,
            pending: root.page.clone(),
            frames: Vec::new(),
            lower: lower.to_vec(),
            upper: upper.map(ToOwned::to_owned),
            budget,
        })
    }

    /// Reads at most `max_rows` ordered entries without scanning the suffix.
    pub(crate) fn map_scan_limit(
        &self,
        root: &MapRoot,
        lower: &[u8],
        upper: Option<&[u8]>,
        max_rows: usize,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<(Vec<u8>, BlobRef)>, CoreError> {
        let mut cursor = self.map_cursor(root, lower, upper, budget)?;
        let mut rows = Vec::new();
        while rows.len() < max_rows {
            let Some(row) = cursor.next()? else { break };
            rows.try_reserve(1)
                .map_err(|error| storage("reserve limited map scan", error))?;
            rows.push(row);
        }
        Ok(rows)
    }

    fn load_page(
        &self,
        reference: &PageRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Page, CoreError> {
        checkpoint(budget)?;
        if reference.object.kind != ObjectKind::MapPage
            || reference.entries == 0
            || reference.first > reference.last
            || reference.height > MAX_MAP_HEIGHT
        {
            return Err(corrupt("map root or page descriptor is invalid"));
        }
        let (dependencies, payload) = self.read_object(&reference.object, budget)?;
        decode_page(&payload, reference, &dependencies)
    }

    pub(super) fn load_page_shared(
        &self,
        reference: &PageRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Arc<Page>, CoreError> {
        checkpoint(budget)?;
        if reference.object.kind != ObjectKind::MapPage
            || reference.entries == 0
            || reference.first > reference.last
            || reference.height > MAX_MAP_HEIGHT
        {
            return Err(corrupt("map root or page descriptor is invalid"));
        }
        if let Some(page) = self.cached_decoded_map_page(reference)? {
            return Ok(page);
        }
        let (dependencies, payload) = self.read_object(&reference.object, budget)?;
        let page = Arc::new(decode_page(&payload, reference, &dependencies)?);
        self.remember_decoded_map_page(reference, &page)?;
        Ok(page)
    }

    fn save_page(&self, lease: &AttemptLease, page: &Page) -> Result<PageRef, CoreError> {
        let (payload, dependencies, first, last, entries, height) = encode_page(page)?;
        let object =
            self.put_adopted_object(lease, ObjectKind::MapPage, &dependencies, &payload)?;
        self.record_map_page_mutation();
        Ok(PageRef {
            object,
            first,
            last,
            entries,
            height,
        })
    }

    fn validate_root(root: &MapRoot) -> Result<(), CoreError> {
        match &root.page {
            None if root.entries == 0 => Ok(()),
            Some(page)
                if page.entries == root.entries
                    && root.entries > 0
                    && page.object.kind == ObjectKind::MapPage =>
            {
                Ok(())
            }
            _ => Err(corrupt("map root count or page reference differs")),
        }
    }

    pub(crate) fn map_get(
        &self,
        root: &MapRoot,
        key: &[u8],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<BlobRef>, CoreError> {
        check_key(key)?;
        Self::validate_root(root)?;
        let Some(mut reference) = root.page.clone() else {
            return Ok(None);
        };
        loop {
            match self.load_page_shared(&reference, budget)?.as_ref() {
                Page::Leaf(rows) => {
                    return rows
                        .binary_search_by(|row| row.0.as_slice().cmp(key))
                        .map_or(Ok(None), |index| {
                            rows.get(index)
                                .map(|row| Some(row.1.clone()))
                                .ok_or_else(|| invalid("map search returned absent row"))
                        });
                }
                Page::Branch(children) => {
                    let found = children.iter().find(|child| {
                        key >= child.first.as_slice() && key <= child.last.as_slice()
                    });
                    let Some(child) = found else { return Ok(None) };
                    reference = child.clone();
                }
            }
        }
    }

    pub(crate) fn map_scan(
        &self,
        root: &MapRoot,
        lower: &[u8],
        upper: Option<&[u8]>,
        budget: Option<&RequestBudgetV1>,
        visitor: &mut ScanVisitor<'_>,
    ) -> Result<(), CoreError> {
        check_key(lower)?;
        if let Some(upper) = upper {
            check_key(upper)?;
            if upper <= lower {
                return Err(invalid("map scan upper bound is not above lower"));
            }
        }
        Self::validate_root(root)?;
        if let Some(page) = &root.page {
            self.scan_page(page, lower, upper, budget, visitor)?;
        }
        Ok(())
    }

    fn scan_page(
        &self,
        reference: &PageRef,
        lower: &[u8],
        upper: Option<&[u8]>,
        budget: Option<&RequestBudgetV1>,
        visitor: &mut ScanVisitor<'_>,
    ) -> Result<(), CoreError> {
        checkpoint(budget)?;
        let page = self.load_page(reference, budget)?;
        match page {
            Page::Leaf(rows) => {
                for (key, value) in &rows {
                    if key.as_slice() >= lower && upper.is_none_or(|upper| key.as_slice() < upper) {
                        visitor(key, value)?;
                    }
                }
            }
            Page::Branch(children) => {
                for child in &children {
                    if child.last.as_slice() < lower
                        || upper.is_some_and(|upper| child.first.as_slice() >= upper)
                    {
                        continue;
                    }
                    self.scan_page(child, lower, upper, budget, visitor)?;
                }
            }
        }
        Ok(())
    }

    /// Sorted batch mutation touches each affected leaf and ancestor once.
    pub(crate) fn map_apply(
        &self,
        lease: &AttemptLease,
        base: &MapRoot,
        sorted_edits: impl Iterator<Item = Result<MapEdit, CoreError>>,
    ) -> Result<MapRoot, CoreError> {
        lease.ensure_store(self)?;
        Self::validate_root(base)?;
        if let Some(page) = &base.page {
            self.adopt_existing_root(lease, &page.object)?;
        }
        let mut edits = EditCursor::new(sorted_edits);
        if edits.peek()?.is_none() {
            return Ok(base.clone());
        }
        let mut levels = Vec::new();
        if let Some(page) = &base.page {
            self.apply_page_stream(lease, page, None, &mut edits, &mut |page_ref| {
                self.push_level(lease, &mut levels, usize::from(page.height), page_ref)
            })?;
        } else {
            let mut leaf = Vec::new();
            while edits.peek()?.is_some() {
                let edit = edits.take()?;
                if let Some(value) = edit.value {
                    self.push_leaf_row(lease, &mut leaf, (edit.key, value), &mut |page_ref| {
                        self.push_level(lease, &mut levels, 0, page_ref)
                    })?;
                }
            }
            self.flush_leaf(lease, &mut leaf, &mut |page_ref| {
                self.push_level(lease, &mut levels, 0, page_ref)
            })?;
        }
        let result = self.root_from_levels(lease, &mut levels)?;
        if let Some(page) = &result.page {
            self.adopt_existing_root(lease, &page.object)?;
        }
        Ok(result)
    }

    fn apply_page_stream<I: Iterator<Item = Result<MapEdit, CoreError>>>(
        &self,
        lease: &AttemptLease,
        reference: &PageRef,
        upper: Option<&[u8]>,
        edits: &mut EditCursor<I>,
        emit: &mut dyn FnMut(PageRef) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if !edits.has_before(upper)? {
            return emit(reference.clone());
        }
        match self.load_page(reference, None)? {
            Page::Leaf(rows) => {
                let mut leaf = Vec::new();
                let mut old = rows.into_iter().peekable();
                while old.peek().is_some() || edits.has_before(upper)? {
                    let ordering = match (old.peek(), edits.peek()?) {
                        (Some((old_key, _)), Some(mutation))
                            if upper.is_none_or(|limit| mutation.key.as_slice() <= limit) =>
                        {
                            Some(old_key.as_slice().cmp(mutation.key.as_slice()))
                        }
                        (Some(_), _) => None,
                        (None, Some(_)) => Some(std::cmp::Ordering::Greater),
                        (None, None) => break,
                    };
                    match ordering {
                        None | Some(std::cmp::Ordering::Less) => {
                            let row = old.next().ok_or_else(|| invalid("old leaf iterator"))?;
                            self.push_leaf_row(lease, &mut leaf, row, emit)?;
                        }
                        Some(std::cmp::Ordering::Equal) => {
                            let _old = old.next();
                            let mutation = edits.take()?;
                            if let Some(value) = mutation.value {
                                self.push_leaf_row(lease, &mut leaf, (mutation.key, value), emit)?;
                            }
                        }
                        Some(std::cmp::Ordering::Greater) => {
                            let mutation = edits.take()?;
                            if let Some(value) = mutation.value {
                                self.push_leaf_row(lease, &mut leaf, (mutation.key, value), emit)?;
                            }
                        }
                    }
                }
                self.flush_leaf(lease, &mut leaf, emit)
            }
            Page::Branch(children) => {
                let mut branch = Vec::new();
                for (index, child) in children.iter().enumerate() {
                    let child_upper = if index.checked_add(1) == Some(children.len()) {
                        upper
                    } else {
                        Some(child.last.as_slice())
                    };
                    self.apply_page_stream(lease, child, child_upper, edits, &mut |page_ref| {
                        branch
                            .try_reserve(1)
                            .map_err(|error| storage("reserve edited branch", error))?;
                        branch.push(page_ref);
                        if branch.len() == BRANCH_FANOUT {
                            let page =
                                self.save_page(lease, &Page::Branch(std::mem::take(&mut branch)))?;
                            emit(page)?;
                        }
                        Ok(())
                    })?;
                }
                if !branch.is_empty() {
                    emit(self.save_page(lease, &Page::Branch(branch))?)?;
                }
                Ok(())
            }
        }
    }

    fn push_leaf_row(
        &self,
        lease: &AttemptLease,
        leaf: &mut Vec<(Vec<u8>, BlobRef)>,
        row: (Vec<u8>, BlobRef),
        emit: &mut dyn FnMut(PageRef) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        leaf.try_reserve(1)
            .map_err(|error| storage("reserve map leaf", error))?;
        leaf.push(row);
        if leaf.len() == LEAF_FANOUT {
            emit(self.save_page(lease, &Page::Leaf(std::mem::take(leaf)))?)?;
        }
        Ok(())
    }

    fn flush_leaf(
        &self,
        lease: &AttemptLease,
        leaf: &mut Vec<(Vec<u8>, BlobRef)>,
        emit: &mut dyn FnMut(PageRef) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if !leaf.is_empty() {
            emit(self.save_page(lease, &Page::Leaf(std::mem::take(leaf)))?)?;
        }
        Ok(())
    }

    fn root_from_levels(
        &self,
        lease: &AttemptLease,
        levels: &mut Vec<Vec<PageRef>>,
    ) -> Result<MapRoot, CoreError> {
        let mut height = 0;
        while height < levels.len() {
            let pending = std::mem::take(
                levels
                    .get_mut(height)
                    .ok_or_else(|| invalid("map level absent"))?,
            );
            if pending.is_empty() {
                height = height
                    .checked_add(1)
                    .ok_or_else(|| invalid("map height overflow"))?;
                continue;
            }
            let higher_exists = levels
                .iter()
                .skip(
                    height
                        .checked_add(1)
                        .ok_or_else(|| invalid("map height overflow"))?,
                )
                .any(|level| !level.is_empty());
            if pending.len() == 1 && !higher_exists {
                let page = pending
                    .into_iter()
                    .next()
                    .ok_or_else(|| invalid("map root absent"))?;
                return Ok(MapRoot {
                    entries: page.entries,
                    page: Some(page),
                });
            }
            for group in pending.chunks(BRANCH_FANOUT) {
                let page = self.save_page(lease, &Page::Branch(group.to_vec()))?;
                self.push_level(
                    lease,
                    levels,
                    height
                        .checked_add(1)
                        .ok_or_else(|| invalid("map height overflow"))?,
                    page,
                )?;
            }
            height = height
                .checked_add(1)
                .ok_or_else(|| invalid("map height overflow"))?;
        }
        Ok(MapRoot::default())
    }

    /// Stream a full ordered bootstrap with O(height × fanout) page refs.
    pub(crate) fn map_bootstrap(
        &self,
        lease: &AttemptLease,
        rows: impl Iterator<Item = Result<MapEdit, CoreError>>,
    ) -> Result<MapRoot, CoreError> {
        lease.ensure_store(self)?;
        let mut leaf = Vec::new();
        let mut levels: Vec<Vec<PageRef>> = Vec::new();
        let mut previous: Option<Vec<u8>> = None;
        for row in rows {
            let row = row?;
            check_key(&row.key)?;
            if previous
                .as_ref()
                .is_some_and(|key| key.as_slice() >= row.key.as_slice())
            {
                return Err(invalid("bootstrap keys are not strictly sorted"));
            }
            previous = Some(row.key.clone());
            let value = row
                .value
                .ok_or_else(|| invalid("bootstrap cannot contain deletion"))?;
            leaf.try_reserve(1)
                .map_err(|error| storage("reserve bootstrap leaf", error))?;
            leaf.push((row.key, value));
            if leaf.len() == LEAF_FANOUT {
                let page = self.save_page(lease, &Page::Leaf(std::mem::take(&mut leaf)))?;
                self.push_level(lease, &mut levels, 0, page)?;
            }
        }
        if !leaf.is_empty() {
            let page = self.save_page(lease, &Page::Leaf(leaf))?;
            self.push_level(lease, &mut levels, 0, page)?;
        }
        let result = self.root_from_levels(lease, &mut levels)?;
        if let Some(page) = &result.page {
            self.adopt_existing_root(lease, &page.object)?;
        }
        Ok(result)
    }

    fn push_level(
        &self,
        lease: &AttemptLease,
        levels: &mut Vec<Vec<PageRef>>,
        height: usize,
        page: PageRef,
    ) -> Result<(), CoreError> {
        if height > usize::from(MAX_MAP_HEIGHT) {
            return Err(invalid("bootstrap map height exceeded"));
        }
        while levels.len() <= height {
            levels
                .try_reserve(1)
                .map_err(|error| storage("reserve map levels", error))?;
            levels.push(Vec::new());
        }
        let level = levels
            .get_mut(height)
            .ok_or_else(|| invalid("map level absent"))?;
        level
            .try_reserve(1)
            .map_err(|error| storage("reserve map page refs", error))?;
        level.push(page);
        if level.len() == BRANCH_FANOUT {
            let children = std::mem::take(level);
            let parent = self.save_page(lease, &Page::Branch(children))?;
            self.push_level(
                lease,
                levels,
                height
                    .checked_add(1)
                    .ok_or_else(|| invalid("map height overflow"))?,
                parent,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod compact_leaf_wire_tests {
    use super::{
        BLOB_REF_WIRE_BYTES, LEAF_DICTIONARY, LEAF_INLINE, Page, add_blob_ref, add_key,
        decode_page, encode_page,
    };
    use crate::lexical_store::codec::PageRef;
    use crate::lexical_store::{BlobRef, ObjectKind, ObjectRef};
    use quanta_index_core::CoreError;

    fn value(seed: u8) -> BlobRef {
        BlobRef {
            root: ObjectRef {
                kind: ObjectKind::Record,
                bytes: 22,
                sha256: [seed; 32],
            },
            bytes: 1,
            sha256: [seed; 32],
        }
    }

    fn leaf(rows: usize, unique: usize) -> Page {
        Page::Leaf(
            (0..rows)
                .map(|index| {
                    (
                        u16::try_from(index)
                            .expect("bounded key")
                            .to_be_bytes()
                            .to_vec(),
                        value(
                            u8::try_from(index.checked_rem(unique).expect("nonzero unique count"))
                                .expect("bounded value"),
                        ),
                    )
                })
                .collect(),
        )
    }

    fn wire(page: &Page) -> (Vec<u8>, PageRef, Vec<ObjectRef>) {
        let (bytes, mut deps, first, last, entries, height) =
            encode_page(page).expect("canonical page");
        deps.sort();
        deps.dedup();
        (
            bytes,
            PageRef {
                object: ObjectRef {
                    kind: ObjectKind::MapPage,
                    bytes: 0,
                    sha256: [99; 32],
                },
                first,
                last,
                entries,
                height,
            },
            deps,
        )
    }

    fn refused(bytes: &[u8], descriptor: &PageRef, deps: &[ObjectRef]) {
        assert!(matches!(
            decode_page(bytes, descriptor, deps),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
        ));
    }

    #[test]
    fn shared_leaf_has_exact_fixed_wire_saving_and_unique_leaf_stays_inline() {
        let page = leaf(96, 1);
        let (bytes, descriptor, deps) = wire(&page);
        assert_eq!(bytes.get(27), Some(&LEAF_DICTIONARY));
        assert_eq!(bytes.len(), 687);
        // A 96-row V2 inline page with two-byte keys is exactly 8,188 bytes.
        assert_eq!(BLOB_REF_WIRE_BYTES, 81);
        assert_eq!(8_188_usize.saturating_sub(bytes.len()), 7_501);
        assert_eq!(
            decode_page(&bytes, &descriptor, &deps).expect("decode"),
            page
        );

        let unique = leaf(96, 96);
        let (bytes, descriptor, deps) = wire(&unique);
        assert_eq!(bytes.get(27), Some(&LEAF_INLINE));
        assert_eq!(
            decode_page(&bytes, &descriptor, &deps).expect("decode"),
            unique
        );
    }

    #[test]
    fn equal_size_uses_inline_and_one_fewer_distinct_value_uses_dictionary() {
        // N=80,U=78 is exact equality: 2+81*78+2*80 == 81*80.
        let (tie, descriptor, deps) = wire(&leaf(80, 78));
        assert_eq!(tie.get(27), Some(&LEAF_INLINE));
        assert!(decode_page(&tie, &descriptor, &deps).is_ok());
        let (smaller, descriptor, deps) = wire(&leaf(80, 77));
        assert_eq!(smaller.get(27), Some(&LEAF_DICTIONARY));
        assert!(decode_page(&smaller, &descriptor, &deps).is_ok());
    }

    #[test]
    fn forged_leaf_dictionary_and_legacy_magic_are_typed_refusals() {
        let page = leaf(96, 2);
        let (bytes, descriptor, deps) = wire(&page);
        assert_eq!(bytes.get(27), Some(&LEAF_DICTIONARY));

        let mut duplicate = bytes.clone();
        let first = duplicate
            .get(30..111)
            .expect("first dictionary value")
            .to_vec();
        duplicate
            .get_mut(111..192)
            .expect("second dictionary value")
            .copy_from_slice(&first);
        refused(&duplicate, &descriptor, &deps);

        let mut unused = bytes.clone();
        for index in 0..96_usize {
            let offset = 192_usize.saturating_add(index.saturating_mul(6));
            unused
                .get_mut(offset.saturating_add(4)..offset.saturating_add(6))
                .expect("row dictionary index")
                .copy_from_slice(&0_u16.to_be_bytes());
        }
        refused(&unused, &descriptor, &deps);

        let mut out_of_range = bytes.clone();
        out_of_range
            .get_mut(196..198)
            .expect("first row dictionary index")
            .copy_from_slice(&2_u16.to_be_bytes());
        refused(&out_of_range, &descriptor, &deps);

        for count in [0_u16, 97_u16] {
            let mut invalid_count = bytes.clone();
            invalid_count
                .get_mut(28..30)
                .expect("dictionary count")
                .copy_from_slice(&count.to_be_bytes());
            refused(&invalid_count, &descriptor, &deps);
        }

        let mut unknown_mode = bytes.clone();
        *unknown_mode.get_mut(27).expect("leaf mode") = 2;
        refused(&unknown_mode, &descriptor, &deps);

        let mut legacy = bytes.clone();
        *legacy.get_mut(7).expect("magic version") = b'1';
        refused(&legacy, &descriptor, &deps);
        refused(
            bytes
                .get(..bytes.len().saturating_sub(1))
                .expect("truncated"),
            &descriptor,
            &deps,
        );
        let mut trailing = bytes;
        trailing.push(0);
        refused(&trailing, &descriptor, &deps);
    }

    #[test]
    fn forged_nonminimal_modes_are_typed_refusals() {
        let page = leaf(96, 1);
        let (bytes, descriptor, deps) = wire(&page);
        let mut inline = bytes.get(..27).expect("fixed header").to_vec();
        inline.push(LEAF_INLINE);
        if let Page::Leaf(rows) = &page {
            for (key, reference) in rows {
                add_key(&mut inline, key).expect("bounded key");
                add_blob_ref(&mut inline, reference);
            }
        }
        refused(&inline, &descriptor, &deps);

        // N=80,U=78 ties, so even an otherwise well-shaped dictionary tag
        // must be refused before the decoder consumes its table.
        let (tie, descriptor, deps) = wire(&leaf(80, 78));
        let mut dictionary = tie;
        *dictionary.get_mut(27).expect("leaf mode") = LEAF_DICTIONARY;
        drop(dictionary.splice(28..28, 78_u16.to_be_bytes()));
        refused(&dictionary, &descriptor, &deps);
    }
}
