//! Independent, bounded reader for the P0 document-partitioned FULL wire.
//! All decoded values are DATA. The selected Source ticket remains authority.

use super::super::{
    PostingOccurrence, PostingStageSample, TextReadContext, TextRecipe, ValidatedPostingHit,
    validate_segment_document, validate_universe_source_reference,
};
use super::wire::{self, Doc, SegmentEdge};
use crate::native::text_path::PathIndex;
use quanta_index_core::index::{
    Digest, IndexError, MemberBinding, MemoryLifetime, ObjectRef, OwnerKey, SourceCut,
    StoredObject, WorkBudget, WorkUsage, ports::ImmutableObjectPort,
};

mod read_v2;
pub(in crate::native::text) use read_v2::TermFrequencies;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct Summary {
    pub docs: u64,
    pub indexed_tokens: u64,
    pub path: PathIndex,
}

struct Root {
    object: StoredObject,
    cut: wire::TextCut,
    rows: usize,
    row_start: usize,
    summary: Summary,
}

impl Root {
    fn edge(&self, index: usize) -> Result<SegmentEdge, IndexError> {
        if index >= self.rows {
            return Err(wire::corrupt("root segment ordinal"));
        }
        let start = self
            .row_start
            .checked_add(
                index
                    .checked_mul(wire::ROOT_ROW_BYTES)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let end = start
            .checked_add(wire::ROOT_ROW_BYTES)
            .ok_or(IndexError::ResourceLimited)?;
        let mut d = Reader::new(
            self.object
                .payload
                .get(start..end)
                .ok_or_else(|| wire::corrupt("root row bounds"))?,
        );
        Ok(SegmentEdge {
            reference: d.reference()?,
            first: d.fixed()?,
            last: d.fixed()?,
            docs: d.u16()?,
            tokens: d.u64()?,
        })
    }
}

fn open_root(
    artifact: ObjectRef,
    recipe: TextRecipe,
    exact_source: Option<SourceCut>,
    expected_logical: Option<Digest>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Root, IndexError> {
    budget.checkpoint()?;
    let object = objects.get(artifact, budget)?;
    if object.registration.reference != artifact {
        return Err(wire::corrupt("root identity"));
    }
    charge_decode(&object, budget)?;
    if let Some(refusal) = super::retired::refuse_retired_root(
        &object.payload,
        recipe.output_policy(),
        &object.registration.children,
        expected_logical,
    ) {
        return Err(refusal);
    }
    let mut d = Reader::new(&object.payload);
    if d.fixed::<8>()? != *wire::ROOT_MAGIC || d.fixed::<32>()? != recipe.output_policy().0 {
        return Err(wire::corrupt("root policy"));
    }
    let cut = d.text_cut()?;
    if exact_source.is_some_and(|expected| wire::TextCut::of(expected) != cut) {
        return Err(IndexError::Conflict);
    }
    let count = usize::from(d.u16()?);
    if count > wire::MAX_SEGMENTS {
        return Err(wire::corrupt("root segment count"));
    }
    let row_start = d.offset;
    let source_count = cut.edges().count();
    if object.registration.children.len() != source_count + count
        || !object
            .registration
            .children
            .get(..source_count)
            .ok_or_else(|| wire::corrupt("root source edges"))?
            .iter()
            .copied()
            .eq(cut.edges())
    {
        return Err(wire::corrupt("root source edges"));
    }
    let mut docs = 0u64;
    let mut tokens = 0u64;
    let mut last = None;
    for slot in 0..count {
        budget.checkpoint()?;
        let reference = d.reference()?;
        let first: [u8; 32] = d.fixed()?;
        let end: [u8; 32] = d.fixed()?;
        let row_docs = d.u16()?;
        let row_tokens = d.u64()?;
        if row_docs == 0
            || usize::from(row_docs) > wire::MAX_DOCS
            || first > end
            || last.is_some_and(|prior| first <= prior)
            || object.registration.children.get(source_count + slot) != Some(&reference)
        {
            return Err(wire::corrupt("root segment fence or edge"));
        }
        last = Some(end);
        docs = docs
            .checked_add(u64::from(row_docs))
            .ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(row_tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if !d.empty() {
        return Err(wire::corrupt("root trailing bytes"));
    }
    if expected_logical.is_some_and(|expected| {
        expected != wire::logical_commitment(recipe.output_policy(), cut, docs, tokens)
    }) {
        return Err(wire::corrupt("root logical commitment"));
    }
    Ok(Root {
        object,
        cut,
        rows: count,
        row_start,
        summary: Summary {
            docs,
            indexed_tokens: tokens,
            path: PathIndex::default(),
        },
    })
}

pub(in crate::native::text) fn root(
    member: &MemberBinding,
    recipe: TextRecipe,
    exact_source: Option<SourceCut>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Summary, IndexError> {
    if recipe.is_layered() {
        return read_v2::root(member, recipe, exact_source, objects, budget);
    }
    if member.input.policy != recipe.output_policy()
        || member.input.root != exact_source.map_or(member.input.root, |source| source.owners)
    {
        return Err(IndexError::Conflict);
    }
    let opened = open_root(
        member.artifact,
        recipe,
        exact_source,
        Some(member.logical_commitment),
        objects,
        budget,
    )?;
    if opened.cut.owners != member.input.root {
        return Err(IndexError::Conflict);
    }
    Ok(opened.summary)
}

/// Independently decode every installed child before the FULL member is sealed.
/// The caller's attempt budget pays for each object read and decoded byte.
pub(in crate::native::text) fn verify_full(
    member: &MemberBinding,
    recipe: TextRecipe,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if recipe.is_layered() {
        return read_v2::verify_full(member, recipe, objects, budget);
    }
    if member.input.policy != recipe.output_policy() {
        return Err(IndexError::Conflict);
    }
    let root = open_root(
        member.artifact,
        recipe,
        None,
        Some(member.logical_commitment),
        objects,
        budget,
    )?;
    if root.cut.owners != member.input.root {
        return Err(IndexError::Conflict);
    }
    for slot in 0..root.rows {
        budget.checkpoint()?;
        let segment = open_segment(root.edge(slot)?, recipe, objects, budget)?;
        verify_v2_segment(&segment, objects, budget)?;
    }
    Ok(())
}

pub(super) fn verify_v2_segment(
    segment: &Segment,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut case_tokens = [[0u64; wire::MAX_DOCS]; 2];
    let mut routes = Reader::at(&segment.object.payload, segment.route_start);
    let mut previous_key: Option<&[u8]> = None;
    let mut previous_boundary: Option<PageBoundary> = None;
    for _ in 0..segment.routes {
        budget.checkpoint()?;
        let route = Route::decode(&mut routes)?;
        let (_, _, boundary) = inspect_page(
            &route,
            None,
            segment,
            Some(&mut case_tokens),
            false,
            objects,
            budget,
        )?;
        if previous_key == Some(route.first)
            && previous_boundary.is_some_and(|prior| boundary.first_doc <= prior.last_doc)
        {
            return Err(wire::corrupt("posting continuation docID order"));
        }
        previous_key = Some(route.last);
        previous_boundary = Some(boundary);
    }
    for docid in 0..segment.docs {
        budget.checkpoint()?;
        let expected = segment.tokens(docid)?;
        if case_tokens
            .iter()
            .any(|counts| counts.get(docid) != Some(&expected))
        {
            return Err(wire::corrupt("posting counts differ from document tokens"));
        }
    }
    Ok(())
}

/// Replay canonical posting pages while emitting only live document positions.
/// The page is fetched and checked once; the callback reads from that same
/// authenticated payload after the checked decoder establishes its bounds.
pub(super) fn replay_v2_postings(
    segment: &Segment,
    live_mask: u128,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(&[u8], usize, u32, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    if segment.docs < 128 && live_mask >> segment.docs != 0 {
        return Err(wire::corrupt("V2 replay mask exceeds document table"));
    }
    let mut case_tokens = [[0u64; wire::MAX_DOCS]; 2];
    let mut routes = Reader::at(&segment.object.payload, segment.route_start);
    let mut previous_key: Option<&[u8]> = None;
    let mut previous_boundary: Option<PageBoundary> = None;
    for _ in 0..segment.routes {
        budget.checkpoint()?;
        let route = Route::decode(&mut routes)?;
        let (page, _, boundary) = inspect_page(
            &route,
            None,
            segment,
            Some(&mut case_tokens),
            false,
            objects,
            budget,
        )?;
        if previous_key == Some(route.first)
            && previous_boundary.is_some_and(|prior| boundary.first_doc <= prior.last_doc)
        {
            return Err(wire::corrupt("posting continuation docID order"));
        }
        previous_key = Some(route.last);
        previous_boundary = Some(boundary);
        let mut page_reader = Reader::new(&page.payload);
        let _magic = page_reader.fixed::<8>()?;
        while !page_reader.empty() {
            budget.checkpoint()?;
            let key = page_reader.key()?;
            let df = page_reader.u16()?;
            let body_len =
                usize::try_from(page_reader.u32()?).map_err(|_| IndexError::ResourceLimited)?;
            let body = page_reader.take(body_len)?;
            let mut postings = Reader::new(body);
            let mut last_doc: Option<u32> = None;
            for index in 0..usize::from(df) {
                budget.checkpoint()?;
                let delta = postings.varint()?;
                let docid = if index == 0 {
                    delta
                } else {
                    last_doc
                        .ok_or_else(|| wire::corrupt("V2 replay docID predecessor"))?
                        .checked_add(delta)
                        .ok_or(IndexError::ResourceLimited)?
                };
                last_doc = Some(docid);
                let doc_index = usize::try_from(docid).map_err(|_| IndexError::ResourceLimited)?;
                let live = live_mask & document_bit(doc_index)? != 0;
                let tf =
                    usize::try_from(postings.varint()?).map_err(|_| IndexError::ResourceLimited)?;
                let mut position: Option<u32> = None;
                for ordinal in 0..tf {
                    budget.checkpoint()?;
                    let value = postings.varint()?;
                    let absolute = if ordinal == 0 {
                        value
                    } else {
                        position
                            .ok_or_else(|| wire::corrupt("V2 replay position predecessor"))?
                            .checked_add(value)
                            .ok_or(IndexError::ResourceLimited)?
                    };
                    position = Some(absolute);
                    if live {
                        visit(key, doc_index, absolute, budget)?;
                    }
                }
            }
            if !postings.empty() {
                return Err(wire::corrupt("V2 replay fragment trailing bytes"));
            }
        }
    }
    for docid in 0..segment.docs {
        budget.checkpoint()?;
        let expected = segment.tokens(docid)?;
        if case_tokens
            .iter()
            .any(|counts| counts.get(docid) != Some(&expected))
        {
            return Err(wire::corrupt("posting counts differ from document tokens"));
        }
    }
    Ok(())
}

pub(super) struct Segment {
    reference: ObjectRef,
    first: [u8; 32],
    last: [u8; 32],
    token_total: u64,
    object: StoredObject,
    docs: usize,
    doc_start: usize,
    routes: usize,
    route_start: usize,
}

impl Segment {
    pub(super) fn descriptor(&self) -> SegmentEdge {
        SegmentEdge {
            reference: self.reference,
            first: self.first,
            last: self.last,
            docs: self.docs as u16,
            tokens: self.token_total,
        }
    }

    fn doc_payload(&self, index: usize) -> Result<&[u8], IndexError> {
        if index >= self.docs {
            return Err(wire::corrupt("posting docID exceeds segment"));
        }
        let start = self
            .doc_start
            .checked_add(
                index
                    .checked_mul(wire::DOC_BYTES)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let end = start
            .checked_add(wire::DOC_BYTES)
            .ok_or(IndexError::ResourceLimited)?;
        self.object
            .payload
            .get(start..end)
            .ok_or_else(|| wire::corrupt("segment doc bounds"))
    }

    fn tokens(&self, index: usize) -> Result<u64, IndexError> {
        let encoded = self
            .doc_payload(index)?
            .last_chunk()
            .ok_or_else(|| wire::corrupt("segment doc token width"))?;
        Ok(u64::from_be_bytes(*encoded))
    }

    pub(super) fn doc(&self, index: usize) -> Result<Doc, IndexError> {
        let mut d = Reader::new(self.doc_payload(index)?);
        Ok(Doc {
            owner: d.fixed()?,
            source: d.reference()?,
            tokens: d.u64()?,
        })
    }
}

struct Route<'a> {
    reference: ObjectRef,
    first: &'a [u8],
    last: &'a [u8],
}

impl<'a> Route<'a> {
    fn decode(d: &mut Reader<'a>) -> Result<Self, IndexError> {
        Ok(Self {
            reference: d.reference()?,
            first: d.key()?,
            last: d.key()?,
        })
    }
}

fn open_segment(
    edge: SegmentEdge,
    recipe: TextRecipe,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Segment, IndexError> {
    let segment = open_segment_checked(edge.reference, recipe, objects, budget)?;
    if segment.descriptor() != edge {
        return Err(wire::corrupt("segment edge mismatch"));
    }
    Ok(segment)
}

pub(super) fn open_v2_segment(
    reference: ObjectRef,
    recipe: TextRecipe,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Segment, IndexError> {
    open_segment_checked(reference, recipe.segment_child_recipe(), objects, budget)
}

fn open_segment_checked(
    reference: ObjectRef,
    recipe: TextRecipe,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Segment, IndexError> {
    let object = objects.get(reference, budget)?;
    if object.registration.reference != reference {
        return Err(wire::corrupt("segment identity"));
    }
    charge_decode(&object, budget)?;
    let mut d = Reader::new(&object.payload);
    if d.fixed::<8>()? != *wire::SEGMENT_MAGIC || d.fixed::<32>()? != recipe.output_policy().0 {
        return Err(wire::corrupt("segment policy"));
    }
    let count = usize::from(d.u16()?);
    if count == 0 || count > wire::MAX_DOCS {
        return Err(wire::corrupt("segment doc count"));
    }
    let doc_start = d.offset;
    let mut prior = None;
    let mut first = None;
    let mut tokens = 0u64;
    for _ in 0..count {
        budget.checkpoint()?;
        let owner: [u8; 32] = d.fixed()?;
        let _source = d.reference()?;
        let row_tokens = d.u64()?;
        if prior.is_some_and(|old| owner <= old) {
            return Err(wire::corrupt("segment owner order"));
        }
        if first.is_none() {
            first = Some(owner);
        }
        prior = Some(owner);
        tokens = tokens
            .checked_add(row_tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    let routes = usize::from(d.u16()?);
    if routes > wire::MAX_PAGES || routes != object.registration.children.len() {
        return Err(wire::corrupt("segment route count"));
    }
    let route_start = d.offset;
    let mut prior_last: Option<&[u8]> = None;
    for slot in 0..routes {
        budget.checkpoint()?;
        let reference = d.reference()?;
        let first = d.key()?;
        let last = d.key()?;
        if first > last
            || prior_last.is_some_and(|prior| first < prior)
            || object.registration.children.get(slot) != Some(&reference)
        {
            return Err(wire::corrupt("segment route order or edge"));
        }
        prior_last = Some(last);
    }
    if !d.empty() {
        return Err(wire::corrupt("segment trailing bytes"));
    }
    Ok(Segment {
        reference,
        first: first.ok_or_else(|| wire::corrupt("empty segment"))?,
        last: prior.ok_or_else(|| wire::corrupt("empty segment"))?,
        token_total: tokens,
        object,
        docs: count,
        doc_start,
        routes,
        route_start,
    })
}

pub(in crate::native::text) fn lookup_document(
    artifact: ObjectRef,
    recipe: TextRecipe,
    owner: OwnerKey,
    context: &mut TextReadContext<'_>,
) -> Result<Option<Doc>, IndexError> {
    Ok(DocumentLookup::new(artifact, recipe)
        .locate(owner, context)?
        .map(|(_, doc)| doc))
}

#[derive(Clone, Copy)]
pub(in crate::native::text) struct DocumentLocation {
    edge: SegmentEdge,
    row: usize,
}

fn owner_edge(
    root: &Root,
    key: [u8; 32],
    budget: &WorkBudget,
) -> Result<Option<SegmentEdge>, IndexError> {
    let mut low = 0usize;
    let mut high = root.rows;
    while low < high {
        budget.checkpoint()?;
        let slot = low.midpoint(high);
        let edge = root.edge(slot)?;
        if key < edge.first {
            high = slot;
            continue;
        }
        if key > edge.last {
            low = slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            continue;
        }
        return Ok(Some(edge));
    }
    Ok(None)
}

/// Share the authenticated directory and its last document table across
/// adjacent overlay owners. Each retained object owns its allocation lease.
pub(in crate::native::text) struct DocumentLookup {
    artifact: ObjectRef,
    recipe: TextRecipe,
    root: Option<Root>,
    segment: Option<(SegmentEdge, Segment)>,
    layered: Option<read_v2::DocumentLookup>,
}

impl DocumentLookup {
    pub(in crate::native::text) fn new(artifact: ObjectRef, recipe: TextRecipe) -> Self {
        Self {
            artifact,
            recipe,
            root: None,
            segment: None,
            layered: None,
        }
    }

    pub(in crate::native::text) fn locate(
        &mut self,
        owner: OwnerKey,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<(DocumentLocation, Doc)>, IndexError> {
        if self.recipe.is_layered() {
            let layered = self
                .layered
                .get_or_insert_with(|| read_v2::DocumentLookup::new(self.artifact, self.recipe));
            return layered.locate(owner, context);
        }
        let root = load_document_root(&mut self.root, self.artifact, self.recipe, context)?;
        let key = super::super::owner_key(owner);
        if self
            .segment
            .as_ref()
            .is_none_or(|(edge, _)| key < edge.first || key > edge.last)
        {
            self.segment = owner_edge(root, key, context.budget)?
                .map(|edge| {
                    open_segment(edge, self.recipe, context.objects, context.budget)
                        .map(|segment| (edge, segment))
                })
                .transpose()?;
        }
        let Some((edge, segment)) = &self.segment else {
            return Ok(None);
        };
        let mut low = 0usize;
        let mut high = segment.docs;
        while low < high {
            context.budget.checkpoint()?;
            let row = low.midpoint(high);
            let doc = segment.doc(row)?;
            if doc.owner == key {
                return Ok(Some((DocumentLocation { edge: *edge, row }, doc)));
            }
            if doc.owner > key {
                high = row;
            } else {
                low = row.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
        Ok(None)
    }
}

fn load_document_root<'a>(
    root: &'a mut Option<Root>,
    artifact: ObjectRef,
    recipe: TextRecipe,
    context: &mut TextReadContext<'_>,
) -> Result<&'a Root, IndexError> {
    let source = context.ticket.view.source;
    if root.is_none() {
        *root = Some(open_root(
            artifact,
            recipe,
            Some(source),
            None,
            context.objects,
            context.budget,
        )?);
    }
    root.as_ref()
        .filter(|cached| cached.cut == wire::TextCut::of(source))
        .ok_or(IndexError::Conflict)
}

/// Walk the authenticated document table without opening posting pages.
pub(in crate::native) struct DocumentCursor {
    artifact: ObjectRef,
    recipe: TextRecipe,
    root: Option<Root>,
    segment: Option<Segment>,
    next_segment: usize,
    next_doc: usize,
    layered: Option<read_v2::DocumentCursor>,
}

impl DocumentCursor {
    pub(in crate::native) fn new(artifact: ObjectRef, recipe: TextRecipe) -> Self {
        Self {
            artifact,
            recipe,
            root: None,
            segment: None,
            next_segment: 0,
            next_doc: 0,
            layered: None,
        }
    }

    pub(in crate::native) fn next(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<super::super::ValidatedUniverseOwner>, IndexError> {
        if self.recipe.is_layered() {
            let layered = self
                .layered
                .get_or_insert_with(|| read_v2::DocumentCursor::new(self.artifact, self.recipe));
            return layered.next(context);
        }
        let root = load_document_root(&mut self.root, self.artifact, self.recipe, context)?;
        loop {
            context.budget.checkpoint()?;
            if let Some(segment) = &self.segment
                && self.next_doc < segment.docs
            {
                let doc = segment.doc(self.next_doc)?;
                let owner = validate_segment_document(doc, self.recipe, context)?;
                self.next_doc = self
                    .next_doc
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                return Ok(Some(owner));
            }
            if self.next_segment == root.rows {
                self.segment = None;
                return Ok(None);
            }
            self.segment = Some(open_segment(
                root.edge(self.next_segment)?,
                self.recipe,
                context.objects,
                context.budget,
            )?);
            self.next_segment = self
                .next_segment
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.next_doc = 0;
        }
    }
}

fn charge_decode(object: &StoredObject, budget: &mut WorkBudget) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(object.payload.len()).map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn at(bytes: &'a [u8], offset: usize) -> Self {
        Self { bytes, offset }
    }
    fn empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], IndexError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(IndexError::ResourceLimited)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| wire::corrupt("truncated"))?;
        self.offset = end;
        Ok(value)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], IndexError> {
        self.take(N)?
            .try_into()
            .map_err(|_| wire::corrupt("fixed field"))
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
    fn text_cut(&mut self) -> Result<wire::TextCut, IndexError> {
        fn one(d: &mut Reader<'_>) -> Result<quanta_index_core::index::MapRoot, IndexError> {
            match d.fixed::<1>()? {
                [0] => Ok(quanta_index_core::index::MapRoot(None)),
                [1] => Ok(quanta_index_core::index::MapRoot(Some(d.reference()?))),
                _ => Err(wire::corrupt("source tag")),
            }
        }
        Ok(wire::TextCut {
            owners: one(self)?,
            paths: one(self)?,
        })
    }
    fn key(&mut self) -> Result<&'a [u8], IndexError> {
        let length = usize::from(self.u16()?);
        if length == 0 || length > wire::MAX_KEY {
            return Err(wire::corrupt("key width"));
        }
        let key = self.take(length)?;
        let _case_term = crate::native::text_memory::decode_case_term_key(key)
            .map_err(|_| wire::corrupt("case term key"))?;
        Ok(key)
    }
    fn varint(&mut self) -> Result<u32, IndexError> {
        let mut value = 0u32;
        for shift in (0..=28).step_by(7) {
            let [byte] = self.fixed::<1>()?;
            if shift == 28 && byte > 15 || shift > 0 && byte == 0 {
                return Err(wire::corrupt("noncanonical varint"));
            }
            value |= u32::from(byte & 0x7f) << shift;
            if byte < 128 {
                return Ok(value);
            }
        }
        Err(wire::corrupt("overlong varint"))
    }
}

#[derive(Clone, Copy)]
struct Fragment {
    start: usize,
    end: usize,
    df: u16,
    first_doc: u32,
    last_doc: u32,
    selected_docs: u128,
}

#[derive(Clone, Copy)]
struct PageBoundary {
    first_doc: u32,
    last_doc: u32,
}

fn inspect_page(
    route: &Route<'_>,
    term: Option<&[u8]>,
    segment: &Segment,
    case_tokens: Option<&mut [[u64; wire::MAX_DOCS]; 2]>,
    record_membership: bool,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(StoredObject, Option<Fragment>, PageBoundary), IndexError> {
    inspect_page_impl(
        route,
        term,
        segment,
        case_tokens,
        record_membership,
        None,
        objects,
        budget,
    )
}

fn inspect_page_impl(
    route: &Route<'_>,
    term: Option<&[u8]>,
    segment: &Segment,
    mut case_tokens: Option<&mut [[u64; wire::MAX_DOCS]; 2]>,
    record_membership: bool,
    mut frequencies: Option<&mut [u32; wire::MAX_DOCS]>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(StoredObject, Option<Fragment>, PageBoundary), IndexError> {
    let object = objects.get(route.reference, budget)?;
    if object.registration.reference != route.reference
        || !object.registration.children.is_empty()
        || object.payload.len() > wire::PAGE_BYTES
    {
        return Err(wire::corrupt("posting page identity or bound"));
    }
    charge_decode(&object, budget)?;
    let mut d = Reader::new(&object.payload);
    if d.fixed::<8>()? != *wire::PAGE_MAGIC {
        return Err(wire::corrupt("posting page magic"));
    }
    let mut prior: Option<&[u8]> = None;
    let mut first_key = None;
    let mut selected = None;
    let mut first_page_doc = None;
    let mut last_page_doc = None;
    while !d.empty() {
        budget.checkpoint()?;
        let key = d.key()?;
        let case = usize::from(*key.first().ok_or_else(|| wire::corrupt("empty key"))?);
        if prior.is_some_and(|previous| key <= previous) {
            return Err(wire::corrupt("posting page key order"));
        }
        let _first_key = first_key.get_or_insert(key);
        prior = Some(key);
        let df = d.u16()?;
        let length = usize::try_from(d.u32()?).map_err(|_| IndexError::ResourceLimited)?;
        if df == 0 || usize::from(df) > segment.docs {
            return Err(wire::corrupt("posting fragment DF"));
        }
        let start = d.offset;
        let body = d.take(length)?;
        let mut rows = Reader::new(body);
        let mut last_doc: Option<u32> = None;
        let mut first_doc = None;
        let select = term == Some(key);
        let mut selected_docs = 0u128;
        for index in 0..usize::from(df) {
            budget.checkpoint()?;
            let value = rows.varint()?;
            let docid = if index == 0 {
                value
            } else {
                if value == 0 {
                    return Err(wire::corrupt("posting docID delta"));
                }
                last_doc
                    .ok_or_else(|| wire::corrupt("posting docID predecessor"))?
                    .checked_add(value)
                    .ok_or_else(|| wire::corrupt("posting docID overflow"))?
            };
            let doc_index = usize::try_from(docid).map_err(|_error| IndexError::ResourceLimited)?;
            let doc_tokens = segment.tokens(doc_index)?;
            if record_membership && select {
                selected_docs |= document_bit(doc_index)?;
            }
            let _first_doc = first_doc.get_or_insert(docid);
            last_doc = Some(docid);
            let tf = usize::try_from(rows.varint()?).map_err(|_| IndexError::ResourceLimited)?;
            if tf == 0
                || tf
                    > usize::try_from(super::super::MAX_POSITIONS_PER_TERM)
                        .map_err(|_| IndexError::ResourceLimited)?
                || u64::try_from(tf).map_err(|_error| IndexError::ResourceLimited)? > doc_tokens
            {
                return Err(wire::corrupt("posting term frequency"));
            }
            if select && let Some(frequencies) = frequencies.as_deref_mut() {
                *frequencies
                    .get_mut(doc_index)
                    .ok_or_else(|| wire::corrupt("posting docID"))? =
                    u32::try_from(tf).map_err(|_error| IndexError::ResourceLimited)?;
            }
            if let Some(counts) = case_tokens.as_deref_mut() {
                let count = counts
                    .get_mut(case)
                    .and_then(|case| case.get_mut(doc_index))
                    .ok_or_else(|| wire::corrupt("posting case or docID"))?;
                *count = count
                    .checked_add(u64::try_from(tf).map_err(|_| IndexError::ResourceLimited)?)
                    .ok_or(IndexError::ResourceLimited)?;
                if *count > doc_tokens {
                    return Err(wire::corrupt("posting count exceeds document tokens"));
                }
            }
            let mut position: Option<u32> = None;
            for ordinal in 0..tf {
                let value = rows.varint()?;
                let next = if ordinal == 0 {
                    value
                } else {
                    if value == 0 {
                        return Err(wire::corrupt("posting position delta"));
                    }
                    position
                        .ok_or_else(|| wire::corrupt("posting position predecessor"))?
                        .checked_add(value)
                        .ok_or_else(|| wire::corrupt("posting position overflow"))?
                };
                if u64::from(next) >= super::super::MAX_NFC_BYTES {
                    return Err(wire::corrupt(
                        "posting position exceeds normalized source bound",
                    ));
                }
                position = Some(next);
            }
        }
        if !rows.empty() {
            return Err(wire::corrupt("posting fragment trailing bytes"));
        }
        let fragment_first = first_doc.ok_or_else(|| wire::corrupt("empty fragment"))?;
        let fragment_last = last_doc.ok_or_else(|| wire::corrupt("empty fragment"))?;
        let _first_page_doc = first_page_doc.get_or_insert(fragment_first);
        last_page_doc = Some(fragment_last);
        if select {
            selected = Some(Fragment {
                start,
                end: d.offset,
                df,
                first_doc: fragment_first,
                last_doc: fragment_last,
                selected_docs,
            });
        }
    }
    if first_key != Some(route.first) || prior != Some(route.last) {
        return Err(wire::corrupt("posting route fence"));
    }
    Ok((
        object,
        selected,
        PageBoundary {
            first_doc: first_page_doc.ok_or_else(|| wire::corrupt("empty posting page"))?,
            last_doc: last_page_doc.ok_or_else(|| wire::corrupt("empty posting page"))?,
        },
    ))
}

/// Overlay owners are sorted. Consume at most one packed membership bitmap per
/// touched segment and term, using the same checked decoder as term scans.
pub(in crate::native::text) struct TermMembership<'a> {
    recipe: TextRecipe,
    key: &'a [u8],
    cached: Option<(ObjectRef, u128)>,
}

fn document_bit(row: usize) -> Result<u128, IndexError> {
    let shift = u32::try_from(row).map_err(|_error| IndexError::ResourceLimited)?;
    1u128
        .checked_shl(shift)
        .ok_or_else(|| wire::corrupt("posting membership row"))
}

impl<'a> TermMembership<'a> {
    pub(in crate::native::text) fn new(recipe: TextRecipe, key: &'a [u8]) -> Self {
        Self {
            recipe,
            key,
            cached: None,
        }
    }

    pub(in crate::native::text) fn contains(
        &mut self,
        location: DocumentLocation,
        context: &mut TextReadContext<'_>,
    ) -> Result<bool, IndexError> {
        let reference = location.edge.reference;
        let bit = document_bit(location.row)?;
        if let Some((cached, mask)) = self.cached
            && cached == reference
        {
            return Ok(mask & bit != 0);
        }
        let segment = open_segment(
            location.edge,
            self.recipe.segment_child_recipe(),
            context.objects,
            context.budget,
        )?;
        let mut routes = Reader::at(&segment.object.payload, segment.route_start);
        let mut last_doc = None;
        let mut mask = 0u128;
        for _ in 0..segment.routes {
            context.budget.checkpoint()?;
            let route = Route::decode(&mut routes)?;
            if self.key < route.first {
                break;
            }
            if self.key > route.last {
                continue;
            }
            let (_, fragment, _) = inspect_page(
                &route,
                Some(self.key),
                &segment,
                None,
                true,
                context.objects,
                context.budget,
            )?;
            if let Some(fragment) = fragment {
                if last_doc.is_some_and(|last| fragment.first_doc <= last) {
                    return Err(wire::corrupt("posting continuation docID order"));
                }
                last_doc = Some(fragment.last_doc);
                mask |= fragment.selected_docs;
            }
        }
        self.cached = Some((reference, mask));
        Ok(mask & bit != 0)
    }
}

struct Page {
    object: StoredObject,
    end: usize,
    cursor: usize,
    remaining: u16,
    last_doc: Option<u32>,
}

/// The cursor owns only one immutable page at a time. It performs a complete
/// charged prepass for exact DF because P0 root/route wire has no term summary.
struct V1TermCursor {
    root: Root,
    recipe: TextRecipe,
    key: Vec<u8>,
    _key_memory: Option<quanta_index_core::index::MemoryLease>,
    df: u64,
    visited: u64,
    segment_slot: usize,
    route_slot: usize,
    route_offset: usize,
    segment: Option<Segment>,
    page: Option<Page>,
    last_owner: Option<[u8; 32]>,
    last_fragment_doc: Option<u32>,
}

impl V1TermCursor {
    pub(in crate::native::text) fn new(
        artifact: ObjectRef,
        recipe: TextRecipe,
        key: &[u8],
        context: &mut TextReadContext<'_>,
    ) -> Result<Self, IndexError> {
        if key.is_empty() || key.len() > wire::MAX_KEY {
            return Err(wire::corrupt("query term width"));
        }
        let _canonical = crate::native::text_memory::decode_case_term_key(key)?;
        let root = open_root(
            artifact,
            recipe,
            Some(context.ticket.view.source),
            None,
            context.objects,
            context.budget,
        )?;
        let key_memory = context.budget.reserve_memory(
            u64::try_from(key.len()).map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(key.len())
            .map_err(|_| IndexError::ResourceLimited)?;
        if owned.capacity() > key.len() {
            return Err(IndexError::ResourceLimited);
        }
        owned.extend_from_slice(key);
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut df = 0u64;
        for slot in 0..root.rows {
            context.budget.checkpoint()?;
            let segment = open_segment(root.edge(slot)?, recipe, context.objects, context.budget)?;
            let mut last_doc = None;
            let mut routes = Reader::at(&segment.object.payload, segment.route_start);
            for _ in 0..segment.routes {
                context.budget.checkpoint()?;
                let route = Route::decode(&mut routes)?;
                if route.first <= key && key <= route.last {
                    let (_page, fragment, _boundary) = inspect_page(
                        &route,
                        Some(key),
                        &segment,
                        None,
                        false,
                        context.objects,
                        context.budget,
                    )?;
                    if let Some(fragment) = fragment {
                        if last_doc.is_some_and(|prior| fragment.first_doc <= prior) {
                            return Err(wire::corrupt("posting continuation docID order"));
                        }
                        last_doc = Some(fragment.last_doc);
                        df = df
                            .checked_add(u64::from(fragment.df))
                            .ok_or(IndexError::ResourceLimited)?;
                    }
                }
            }
        }
        if df > root.summary.docs {
            return Err(wire::corrupt("term DF exceeds root docs"));
        }
        Ok(Self {
            root,
            recipe,
            key: owned,
            _key_memory: key_memory,
            df,
            visited: 0,
            segment_slot: 0,
            route_slot: 0,
            route_offset: 0,
            segment: None,
            page: None,
            last_owner: None,
            last_fragment_doc: None,
        })
    }

    pub(in crate::native::text) const fn df(&self) -> u64 {
        self.df
    }

    pub(in crate::native::text) fn next_hit(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        context.budget.checkpoint()?;
        if self.df == 0 {
            return Ok(None);
        }
        loop {
            context.budget.checkpoint()?;
            if self.page.as_ref().is_some_and(|page| page.remaining > 0) {
                return self.read_one(context).map(Some);
            }
            if let Some(page) = self.page.take() {
                if page.cursor != page.end {
                    return Err(wire::corrupt("posting fragment EOF"));
                }
            }
            if self.segment.is_none() {
                if self.segment_slot == self.root.rows {
                    if self.visited != self.df {
                        return Err(wire::corrupt("term DF differs from complete traversal"));
                    }
                    return Ok(None);
                }
                self.segment = Some(open_segment(
                    self.root.edge(self.segment_slot)?,
                    self.recipe,
                    context.objects,
                    context.budget,
                )?);
                self.route_slot = 0;
                self.route_offset = self
                    .segment
                    .as_ref()
                    .ok_or_else(|| wire::corrupt("segment lost"))?
                    .route_start;
                self.last_fragment_doc = None;
            }
            let segment = self
                .segment
                .as_ref()
                .ok_or_else(|| wire::corrupt("segment lost"))?;
            if self.route_slot == segment.routes {
                self.segment = None;
                self.segment_slot = self
                    .segment_slot
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                continue;
            }
            let mut routes = Reader::at(&segment.object.payload, self.route_offset);
            let route = Route::decode(&mut routes)?;
            self.route_offset = routes.offset;
            self.route_slot = self
                .route_slot
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if !(route.first <= self.key.as_slice() && self.key.as_slice() <= route.last) {
                continue;
            }
            let (object, fragment, _boundary) = inspect_page(
                &route,
                Some(&self.key),
                segment,
                None,
                false,
                context.objects,
                context.budget,
            )?;
            let Some(fragment) = fragment else {
                continue;
            };
            if self
                .last_fragment_doc
                .is_some_and(|prior| fragment.first_doc <= prior)
            {
                return Err(wire::corrupt("posting continuation docID order"));
            }
            self.last_fragment_doc = Some(fragment.last_doc);
            self.page = Some(Page {
                object,
                end: fragment.end,
                cursor: fragment.start,
                remaining: fragment.df,
                last_doc: None,
            });
        }
    }

    fn read_one(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<ValidatedPostingHit, IndexError> {
        let page = self
            .page
            .as_mut()
            .ok_or_else(|| wire::corrupt("posting page lost"))?;
        let segment = self
            .segment
            .as_ref()
            .ok_or_else(|| wire::corrupt("posting segment lost"))?;
        let mut d = Reader::at(&page.object.payload, page.cursor);
        let raw_docid = d.varint()?;
        let docid = if let Some(previous) = page.last_doc {
            if raw_docid == 0 {
                return Err(wire::corrupt("posting docID delta"));
            }
            previous
                .checked_add(raw_docid)
                .ok_or_else(|| wire::corrupt("posting docID overflow"))?
        } else {
            raw_docid
        };
        let tf = usize::try_from(d.varint()?).map_err(|_| IndexError::ResourceLimited)?;
        if tf == 0
            || tf
                > usize::try_from(super::super::MAX_POSITIONS_PER_TERM)
                    .map_err(|_| IndexError::ResourceLimited)?
        {
            return Err(wire::corrupt("posting term frequency"));
        }
        let bytes = tf
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(tf)
            .map_err(|_| IndexError::ResourceLimited)?;
        if positions.capacity() > tf {
            return Err(IndexError::ResourceLimited);
        }
        let mut position: Option<u32> = None;
        for index in 0..tf {
            context.budget.checkpoint()?;
            let value = d.varint()?;
            let next = if index == 0 {
                value
            } else {
                if value == 0 {
                    return Err(wire::corrupt("posting position delta"));
                }
                position
                    .ok_or_else(|| wire::corrupt("posting position predecessor"))?
                    .checked_add(value)
                    .ok_or_else(|| wire::corrupt("posting position overflow"))?
            };
            positions.push(next);
            position = Some(next);
        }
        if d.offset > page.end {
            return Err(wire::corrupt("posting fragment bounds"));
        }
        let doc = segment.doc(usize::try_from(docid).map_err(|_| IndexError::ResourceLimited)?)?;
        if u64::try_from(tf).map_err(|_| IndexError::ResourceLimited)? > doc.tokens {
            return Err(wire::corrupt("posting TF exceeds document tokens"));
        }
        let validated = validate_segment_document(doc, self.recipe, context)?;
        if validated.doc_indexed_tokens != doc.tokens
            || self.last_owner.is_some_and(|prior| doc.owner <= prior)
        {
            return Err(wire::corrupt("posting owner or source order"));
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        page.cursor = d.offset;
        page.remaining = page
            .remaining
            .checked_sub(1)
            .ok_or_else(|| wire::corrupt("posting DF underflow"))?;
        page.last_doc = Some(docid);
        self.last_owner = Some(doc.owner);
        self.visited = self
            .visited
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if self.visited > self.df {
            return Err(wire::corrupt("posting exceeds declared DF"));
        }
        Ok(ValidatedPostingHit {
            owner: validated.owner,
            source_record: validated.source_record,
            doc_indexed_tokens: validated.doc_indexed_tokens,
            occurrence: PostingOccurrence::Positions(
                crate::native::text_occurrence::ScopedOccurrence::from_checked_positions(
                    positions, memory,
                ),
            ),
        })
    }
}

pub(in crate::native::text) struct TermCursor(TermCursorKind);

enum TermCursorKind {
    Physical(V1TermCursor),
    Layered(read_v2::TermCursor),
}

impl TermCursor {
    pub(in crate::native::text) fn new(
        artifact: ObjectRef,
        recipe: TextRecipe,
        key: &[u8],
        context: &mut TextReadContext<'_>,
    ) -> Result<Self, IndexError> {
        if recipe.is_layered() {
            Ok(Self(TermCursorKind::Layered(read_v2::TermCursor::new(
                artifact, recipe, key, context,
            )?)))
        } else {
            Ok(Self(TermCursorKind::Physical(V1TermCursor::new(
                artifact, recipe, key, context,
            )?)))
        }
    }

    pub(in crate::native::text) const fn df(&self) -> u64 {
        match &self.0 {
            TermCursorKind::Physical(cursor) => cursor.df(),
            TermCursorKind::Layered(cursor) => cursor.df(),
        }
    }

    pub(in crate::native::text) fn take_stage_probe(&mut self) -> Option<PostingStageSample> {
        match &mut self.0 {
            TermCursorKind::Layered(cursor) => cursor.take_stage_probe(),
            TermCursorKind::Physical(_) => None,
        }
    }

    pub(in crate::native::text) const fn stage_probe_layout(&self) -> &'static str {
        match &self.0 {
            TermCursorKind::Layered(_) => "layered_v2",
            TermCursorKind::Physical(_) => "segment_v1",
        }
    }

    pub(in crate::native::text) fn next_hit(
        &mut self,
        context: &mut TextReadContext<'_>,
        positions_required: bool,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        match &mut self.0 {
            TermCursorKind::Physical(cursor) => cursor.next_hit(context),
            TermCursorKind::Layered(cursor) => cursor.next_hit(context, positions_required),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_wire_varint_rejects_non_minimal_overflow_and_truncation() -> Result<(), IndexError> {
        // Fixed wire vectors; the production encoder is not the oracle.
        for bytes in [
            &[0x80, 0x00][..],
            &[0xff, 0xff, 0xff, 0xff, 0x10][..],
            &[0x80][..],
        ] {
            assert!(matches!(
                Reader::new(bytes).varint(),
                Err(IndexError::CorruptObject(_))
            ));
        }
        assert_eq!(Reader::new(&[0x80, 0x01]).varint()?, 128);
        assert_eq!(
            Reader::new(&[0xff, 0xff, 0xff, 0xff, 0x0f]).varint()?,
            u32::MAX
        );
        Ok(())
    }
}
