//! Enumerate the authenticated Structural corpus membership in owner order.

use quanta_index_contract::{
    ChunkId,
    native_protocol::{
        MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1, MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1,
        NativeDslStructuralChunkMemberV1, NativeDslStructuralChunksCursorV1,
        NativeDslStructuralChunksPageV1,
    },
};
use quanta_index_core::index::MapRoot;

use super::*;

const OWNER_SCAN_LIMIT: u32 = 32;

pub(in super::super) fn structural_chunks_page(
    dispatcher: &NativeDispatcher,
    ticket: &ReadTicket,
    attempt: &TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    scope: &NativeDslScopeV1,
    after: Option<NativeDslStructuralChunksCursorV1>,
    work: &mut WorkBudget,
) -> Result<NativeReply, IndexError> {
    let (projection, corpus) = binding(
        dispatcher,
        ticket,
        attempt,
        scope,
        NativeDslKindV1::Structural,
        work,
    )?;
    let view = ticket.view().identity;
    let source = ticket.view().source;
    let structural_certificate = corpus.certificate;
    if after.is_some_and(|cursor| {
        cursor.view != view
            || cursor.projection != projection
            || cursor.structural_certificate != structural_certificate
            || cursor.owner == NATIVE_DSL_HEADER_OWNER_V1
    }) {
        return Err(IndexError::InvalidInput(
            "Native Structural chunk cursor binding differs".into(),
        ));
    }
    let cursor = |owner, chunk_offset| NativeDslStructuralChunksCursorV1 {
        view,
        projection,
        structural_certificate,
        owner,
        chunk_offset,
    };
    let storage = DslStorage {
        objects: dispatcher.objects.as_ref(),
        maps: dispatcher.objects.as_ref(),
    };
    let head = load_header::<NativeDslCorpusV1>(storage, corpus, work)?;
    if !scope.matches_pin(&head.value.pin) {
        return Err(IndexError::SnapshotUnavailable);
    }
    let chunk_index = publication::chunk_index(storage, corpus, work)?;
    let slot = NativeReply::reserve_canonical(|bytes| {
        dispatcher
            .resources
            .reserve_response_memory(bytes)
            .map(Some)
    })?;
    let row_bytes = MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1
        * std::mem::size_of::<NativeDslStructuralChunkMemberV1>();
    let backing = dispatcher
        .resources
        .reserve_response_memory((MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1 + row_bytes) as u64)?;
    let mut page = ChunkPage {
        chunks: Vec::new(),
        bytes: 0,
    };
    page.chunks
        .try_reserve_exact(MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1)
        .map_err(|_| IndexError::ResourceLimited)?;
    if page.chunks.capacity() > MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1 {
        return Err(IndexError::ResourceLimited);
    }
    let after_key = after.map(|cursor| owner_map_key(cursor.owner));
    let next = 'scan: {
        if let Some(previous) = after {
            let reference = storage
                .maps
                .lookup_object(corpus.owners, &owner_map_key(previous.owner), work)?
                .ok_or_else(|| {
                    IndexError::InvalidInput("Native Structural cursor owner is absent".into())
                })?;
            if let Some(offset) = previous.chunk_offset {
                if let Some(next) = page.append_file(
                    storage,
                    corpus,
                    chunk_index,
                    previous.owner,
                    reference,
                    offset,
                    work,
                )? {
                    break 'scan Some(cursor(previous.owner, Some(next)));
                }
            }
        }
        let rows = storage.maps.scan(
            corpus.owners,
            after_key.as_ref().map(|key| key.as_slice()),
            OWNER_SCAN_LIMIT,
            work,
        )?;
        let has_more = rows.has_more;
        let mut previous_owner = after.map(|cursor| cursor.owner);
        for row in rows.cursor() {
            let (key, reference) = row.object_parts()?;
            if key.as_slice() == publication::CHUNK_INDEX
                || key.as_slice() == owner_map_key(NATIVE_DSL_HEADER_OWNER_V1)
            {
                continue;
            }
            let owner = owner_from_map_key(key)?;
            if let Some(next) =
                page.append_file(storage, corpus, chunk_index, owner, *reference, 0, work)?
            {
                break 'scan Some(cursor(owner, Some(next)));
            }
            previous_owner = Some(owner);
        }
        if has_more {
            Some(cursor(
                previous_owner.ok_or_else(|| corrupt("Structural owner scan made no progress"))?,
                None,
            ))
        } else {
            None
        }
    };
    if next.is_some() && next == after {
        return Err(corrupt("Native Structural chunk scan made no progress"));
    }
    work.checkpoint()?;
    Ok(slot.finish_with_extra_memory(
        NativeIndexResponse::DslStructuralChunksPageV1(NativeDslStructuralChunksPageV1 {
            session,
            view,
            source,
            projection,
            structural_certificate,
            chunks: page.chunks,
            after: next,
        }),
        Some(backing),
    ))
}

struct ChunkPage {
    chunks: Vec<NativeDslStructuralChunkMemberV1>,
    bytes: usize,
}

impl ChunkPage {
    fn append_file(
        &mut self,
        storage: DslStorage<'_>,
        corpus: ProjectionBinding,
        chunk_index: MapRoot,
        owner: OwnerKey,
        reference: ObjectRef,
        offset: u32,
        work: &mut WorkBudget,
    ) -> Result<Option<u32>, IndexError> {
        let record = contribution(storage, corpus, owner, reference, work)?;
        if record.source_owner.map(quanta_index_contract::native_protocol::native_dsl_file_owner_v1)
            != Some(owner)
        {
            return Err(corrupt("Native Structural chunk owner differs from its Source owner"));
        }
        let output = load_output(storage, record, work)?;
        let file = decode::<NativeDslFileV1>(&output, work)?;
        if offset != 0 && offset as usize >= file.value.chunks.len() {
            return Err(IndexError::InvalidInput(
                "Native Structural cursor offset is past its owner".into(),
            ));
        }
        for (index, chunk) in file
            .value
            .chunks
            .into_iter()
            .enumerate()
            .skip(offset as usize)
        {
            let text = chunk.chunk_id.into_inner();
            let text_capacity = text.capacity();
            if text_capacity > MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1 {
                return Err(IndexError::ResourceLimited);
            }
            if self.chunks.len() == MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1
                || text_capacity > MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1 - self.bytes
            {
                return u32::try_from(index)
                    .map(Some)
                    .map_err(|_| IndexError::ResourceLimited);
            }
            work.charge(WorkUsage {
                bytes: (text.len() + std::mem::size_of::<OwnerKey>()) as u64,
                ..WorkUsage::default()
            })?;
            let id = ChunkId::new(text);
            if storage
                .maps
                .lookup_object(chunk_index, &publication::chunk_key(&id), work)?
                != Some(reference)
            {
                return Err(corrupt(
                    "Native Structural chunk index differs from its owner",
                ));
            }
            self.bytes += text_capacity;
            self.chunks.push(NativeDslStructuralChunkMemberV1 { owner, chunk_id: id });
        }
        Ok(None)
    }
}
