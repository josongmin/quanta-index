//! Bounded binary sorted runs for transient Native literal membership edits.
//! A resident vector is preferred; managed SQLite stores only binary pages
//! when its 32 MiB envelope is reached. No scratch row is a published index.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ScratchAllocation, ScratchRequest, ScratchSpacePort,
    WorkBudget, WorkUsage,
};
use quanta_index_scratch_sqlite::ScratchConnection;
use rusqlite::{OptionalExtension as _, params, types::ValueRef};

use super::literal_memberships::MembershipEdit;

const RESIDENT_CHUNK_ENTRIES: usize = 1024 * 1024 / 8;
const RESIDENT_CHUNKS: usize = 32;
const PAGE_ENTRIES: usize = 8192;
const PAGE_BYTES: usize = PAGE_ENTRIES * 8;
const FAN_IN: usize = 8;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const MAX_RUNS: usize = 4096;
const INSERT_SQL: &str = "INSERT INTO run_pages(run,page,payload) VALUES (?1,?2,?3)";
const READ_SQL: &str = "SELECT payload FROM run_pages WHERE run=?1 AND page=?2";

fn sql(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "literal delta runs")
}

fn order(row: MembershipEdit) -> ([u8; 3], u32) {
    (row.gram, row.ordinal)
}

fn validate_sorted(rows: &[MembershipEdit]) -> Result<(), IndexError> {
    if rows.windows(2).any(|pair| order(pair[0]) >= order(pair[1])) {
        return Err(IndexError::BackendViolation(
            "literal delta run has duplicate or unordered keys".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RunMeta {
    id: u32,
    pages: u32,
}

struct RunStore {
    // Connection closes before allocation retires its managed namespace.
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
    runs: Vec<RunMeta>,
    next_id: u32,
    encoded: Vec<u8>,
    _runs_memory: Option<MemoryLease>,
    _encoded_memory: Option<MemoryLease>,
}

impl RunStore {
    fn new(scratch: &dyn ScratchSpacePort, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let allocation = scratch.allocate(
            ScratchRequest {
                initial_bytes: SCRATCH_INITIAL_BYTES,
                max_bytes: SCRATCH_MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != SCRATCH_MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "literal delta scratch envelope changed".into(),
            ));
        }
        let db = ScratchConnection::open(
            &allocation.path().join("literal_delta.sqlite"),
            allocation.as_ref(),
            budget,
        )?;
        {
            let scoped = db.scoped(budget)?;
            scoped
                .execute_batch(
                    "PRAGMA page_size=4096; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
                 PRAGMA temp_store=FILE; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;",
                )
                .map_err(|error| sql(&error))?;
            quanta_index_scratch_sqlite::configure(&scoped, allocation.as_ref())?;
            quanta_index_scratch_sqlite::reserve_for_write(
                &scoped,
                allocation.as_ref(),
                8192,
                budget,
            )?;
            scoped
                .execute_batch(
                    "CREATE TABLE run_pages(run INTEGER NOT NULL,page INTEGER NOT NULL,
                 payload BLOB NOT NULL,PRIMARY KEY(run,page)) WITHOUT ROWID;
                 BEGIN IMMEDIATE;",
                )
                .map_err(|error| sql(&error))?;
        }
        let runs_bytes = u64::try_from(
            MAX_RUNS
                .checked_mul(std::mem::size_of::<RunMeta>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        let runs_memory = budget.reserve_memory(runs_bytes, MemoryLifetime::Temporary)?;
        let encoded_memory = budget.reserve_memory(PAGE_BYTES as u64, MemoryLifetime::Temporary)?;
        let mut runs = Vec::new();
        runs.try_reserve_exact(MAX_RUNS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(PAGE_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            db,
            allocation,
            runs,
            next_id: 0,
            encoded,
            _runs_memory: runs_memory,
            _encoded_memory: encoded_memory,
        })
    }

    fn new_run(&mut self) -> Result<RunMeta, IndexError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(RunMeta { id, pages: 0 })
    }

    fn write_page(
        &mut self,
        run: &mut RunMeta,
        rows: &[MembershipEdit],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if rows.is_empty() || rows.len() > PAGE_ENTRIES {
            return Err(IndexError::ResourceLimited);
        }
        validate_sorted(rows)?;
        self.encoded.clear();
        for row in rows {
            self.encoded.extend_from_slice(&row.ordinal.to_be_bytes());
            self.encoded.extend_from_slice(&row.gram);
            self.encoded.push(u8::from(row.add));
        }
        if self.encoded.len() != rows.len() * 8 || self.encoded.len() > PAGE_BYTES {
            return Err(IndexError::BackendViolation(
                "literal delta page encoding changed".into(),
            ));
        }
        let bytes =
            u64::try_from(self.encoded.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            bytes: bytes
                .checked_add(
                    u64::try_from(INSERT_SQL.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            bytes.checked_add(128).ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        let inserted = db
            .execute(
                INSERT_SQL,
                params![i64::from(run.id), i64::from(run.pages), &self.encoded],
            )
            .map_err(|error| sql(&error))?;
        if inserted != 1 {
            return Err(IndexError::BackendViolation(
                "literal delta page insert count".into(),
            ));
        }
        run.pages = run
            .pages
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    fn write_run(
        &mut self,
        rows: &[MembershipEdit],
        budget: &mut WorkBudget,
    ) -> Result<RunMeta, IndexError> {
        if rows.is_empty() {
            return Err(IndexError::InvalidInput("empty literal delta run".into()));
        }
        validate_sorted(rows)?;
        let mut run = self.new_run()?;
        for page in rows.chunks(PAGE_ENTRIES) {
            self.write_page(&mut run, page, budget)?;
        }
        Ok(run)
    }

    fn read_page(
        &self,
        run: RunMeta,
        page: u32,
        output: &mut Vec<MembershipEdit>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if page >= run.pages {
            return Err(IndexError::ResourceLimited);
        }
        output.clear();
        let db = self.db.scoped(budget)?;
        let found = db
            .query_row(
                READ_SQL,
                params![i64::from(run.id), i64::from(page)],
                |row| {
                    let value = row.get_ref(0)?;
                    let ValueRef::Blob(bytes) = value else {
                        return Ok(Err(IndexError::CorruptObject(
                            "literal delta payload is not BLOB".into(),
                        )));
                    };
                    Ok(decode_page(bytes, output, budget))
                },
            )
            .optional()
            .map_err(|error| sql(&error))?;
        found.ok_or_else(|| IndexError::CorruptObject("literal delta page missing".into()))??;
        Ok(())
    }

    fn delete_run(&self, run: RunMeta, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let db = self.db.scoped(budget)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            4096,
            budget,
        )?;
        budget.charge(WorkUsage {
            writes: u64::from(run.pages),
            bytes: u64::from(run.pages)
                .checked_mul(PAGE_BYTES as u64)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let removed = db
            .execute(
                "DELETE FROM run_pages WHERE run=?1",
                params![i64::from(run.id)],
            )
            .map_err(|error| sql(&error))?;
        if removed != usize::try_from(run.pages).map_err(|_error| IndexError::ResourceLimited)? {
            return Err(IndexError::CorruptObject(
                "literal delta run deletion count".into(),
            ));
        }
        Ok(())
    }
}

fn decode_page(
    bytes: &[u8],
    output: &mut Vec<MembershipEdit>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if bytes.is_empty() || bytes.len() > PAGE_BYTES || bytes.len() % 8 != 0 {
        return Err(IndexError::CorruptObject(
            "literal delta page length".into(),
        ));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    for part in bytes.chunks_exact(8) {
        let ordinal = u32::from_be_bytes(
            part.get(..4)
                .ok_or(IndexError::ResourceLimited)?
                .try_into()
                .map_err(|_error| IndexError::ResourceLimited)?,
        );
        let gram = part
            .get(4..7)
            .ok_or(IndexError::ResourceLimited)?
            .try_into()
            .map_err(|_error| IndexError::ResourceLimited)?;
        let add = match part.get(7).copied() {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(IndexError::CorruptObject("literal delta add bit".into())),
        };
        output.push(MembershipEdit { ordinal, gram, add });
    }
    validate_sorted(output)?;
    Ok(())
}

struct RunReader {
    run: RunMeta,
    next_page: u32,
    page: Vec<MembershipEdit>,
    cursor: usize,
    last: Option<([u8; 3], u32)>,
    _memory: Option<MemoryLease>,
}

impl RunReader {
    fn new(run: RunMeta, budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(PAGE_BYTES as u64, MemoryLifetime::Temporary)?;
        let mut page = Vec::new();
        page.try_reserve_exact(PAGE_ENTRIES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            run,
            next_page: 0,
            page,
            cursor: 0,
            last: None,
            _memory: memory,
        })
    }

    fn next(
        &mut self,
        store: &RunStore,
        budget: &mut WorkBudget,
    ) -> Result<Option<MembershipEdit>, IndexError> {
        if self.cursor == self.page.len() {
            if self.next_page == self.run.pages {
                return Ok(None);
            }
            store.read_page(self.run, self.next_page, &mut self.page, budget)?;
            self.next_page = self
                .next_page
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.cursor = 0;
        }
        let row = *self
            .page
            .get(self.cursor)
            .ok_or(IndexError::ResourceLimited)?;
        if self.last.is_some_and(|previous| previous >= order(row)) {
            return Err(IndexError::CorruptObject(
                "literal delta run page order".into(),
            ));
        }
        self.last = Some(order(row));
        self.cursor = self
            .cursor
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(row))
    }
}

struct ResidentChunk {
    rows: Vec<MembershipEdit>,
    cursor: usize,
    _memory: Option<MemoryLease>,
}

impl ResidentChunk {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            (RESIDENT_CHUNK_ENTRIES * 8) as u64,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(RESIDENT_CHUNK_ENTRIES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            rows,
            cursor: 0,
            _memory: memory,
        })
    }

    fn sort(&mut self) -> Result<(), IndexError> {
        self.rows.sort_unstable_by_key(|row| order(*row));
        validate_sorted(&self.rows)
    }
}

pub(super) struct DeltaCollector {
    resident: Vec<ResidentChunk>,
    active: usize,
    max_chunks: usize,
    _resident_metadata: Option<MemoryLease>,
    store: Option<RunStore>,
    poisoned: bool,
}

impl DeltaCollector {
    pub(super) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let metadata = budget.reserve_memory(
            u64::try_from(
                RESIDENT_CHUNKS
                    .checked_mul(std::mem::size_of::<ResidentChunk>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut resident = Vec::new();
        resident
            .try_reserve_exact(RESIDENT_CHUNKS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            resident,
            active: 0,
            max_chunks: RESIDENT_CHUNKS,
            _resident_metadata: metadata,
            store: None,
            poisoned: false,
        })
    }

    fn spill(
        &mut self,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.active == 0 {
            return Ok(());
        }
        if self.store.is_none() {
            self.store = Some(RunStore::new(scratch, budget)?);
        }
        let store = self.store.as_mut().ok_or(IndexError::ResourceLimited)?;
        for chunk in self.resident.iter_mut().take(self.active) {
            if chunk.rows.is_empty() {
                continue;
            }
            chunk.sort()?;
            if store.runs.len() == MAX_RUNS {
                return Err(IndexError::ResourceLimited);
            }
            let run = store.write_run(&chunk.rows, budget)?;
            store.runs.push(run);
            chunk.rows.clear();
            chunk.cursor = 0;
        }
        self.active = 0;
        Ok(())
    }

    pub(super) fn extend(
        &mut self,
        chunk: &[MembershipEdit],
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "literal delta collector poisoned".into(),
            ));
        }
        let result = (|| {
            for row in chunk {
                if self.active == 0
                    || self
                        .resident
                        .get(self.active - 1)
                        .is_some_and(|last| last.rows.len() == RESIDENT_CHUNK_ENTRIES)
                {
                    if self.active == self.max_chunks {
                        self.spill(scratch, budget)?;
                    }
                    if self.active == self.resident.len() {
                        self.resident.push(ResidentChunk::new(budget)?);
                    }
                    self.active = self
                        .active
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                self.resident
                    .get_mut(self.active - 1)
                    .ok_or(IndexError::ResourceLimited)?
                    .rows
                    .push(*row);
            }
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    pub(super) fn finish(
        mut self,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<DeltaStream, IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "literal delta collector poisoned".into(),
            ));
        }
        if self.store.is_none() {
            for chunk in self.resident.iter_mut().take(self.active) {
                chunk.sort()?;
            }
            let frontier_memory = budget.reserve_memory(
                u64::try_from(
                    RESIDENT_CHUNKS
                        .checked_mul(std::mem::size_of::<ResidentHead>())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            let mut frontier = BinaryHeap::new();
            frontier
                .try_reserve_exact(RESIDENT_CHUNKS)
                .map_err(|_error| IndexError::ResourceLimited)?;
            for (slot, chunk) in self.resident.iter_mut().take(self.active).enumerate() {
                if let Some(row) = chunk.rows.first().copied() {
                    frontier.push(Reverse(ResidentHead { row, slot }));
                    chunk.cursor = 1;
                }
            }
            return Ok(DeltaStream(
                DeltaStreamInner::Resident {
                    chunks: self.resident,
                    frontier,
                    last: None,
                    _metadata: self._resident_metadata,
                    _frontier_memory: frontier_memory,
                },
                false,
            ));
        }
        self.spill(scratch, budget)?;
        let mut store = self.store.take().ok_or(IndexError::ResourceLimited)?;
        store.merge_all(budget)?;
        let run = *store.runs.first().ok_or(IndexError::ResourceLimited)?;
        Ok(DeltaStream(
            DeltaStreamInner::Spilled {
                reader: RunReader::new(run, budget)?,
                store,
            },
            false,
        ))
    }
}

impl RunStore {
    fn merge_all(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        while self.runs.len() > 1 {
            let input_count = self.runs.len();
            let mut output_count = 0;
            for start in (0..input_count).step_by(FAN_IN) {
                let end = start.saturating_add(FAN_IN).min(input_count);
                let mut group = [RunMeta { id: 0, pages: 0 }; FAN_IN];
                let count = end.checked_sub(start).ok_or(IndexError::ResourceLimited)?;
                for (slot, position) in (start..end).enumerate() {
                    *group.get_mut(slot).ok_or(IndexError::ResourceLimited)? =
                        *self.runs.get(position).ok_or(IndexError::ResourceLimited)?;
                }
                let merged = self.merge_group(
                    group.get(..count).ok_or(IndexError::ResourceLimited)?,
                    budget,
                )?;
                *self
                    .runs
                    .get_mut(output_count)
                    .ok_or(IndexError::ResourceLimited)? = merged;
                output_count = output_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            self.runs.truncate(output_count);
        }
        Ok(())
    }

    fn merge_group(
        &mut self,
        group: &[RunMeta],
        budget: &mut WorkBudget,
    ) -> Result<RunMeta, IndexError> {
        let _readers_memory = budget.reserve_memory(
            u64::try_from(
                FAN_IN
                    .checked_mul(std::mem::size_of::<RunReader>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut readers = Vec::new();
        readers
            .try_reserve_exact(FAN_IN)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut heads = [None; FAN_IN];
        for (slot, run) in group.iter().copied().enumerate() {
            let mut reader = RunReader::new(run, budget)?;
            *heads.get_mut(slot).ok_or(IndexError::ResourceLimited)? = reader.next(self, budget)?;
            readers.push(reader);
        }
        let mut output = Vec::new();
        let _output_memory = budget.reserve_memory(PAGE_BYTES as u64, MemoryLifetime::Temporary)?;
        output
            .try_reserve_exact(PAGE_ENTRIES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut target = self.new_run()?;
        let mut last = None;
        loop {
            let chosen = heads
                .iter()
                .enumerate()
                .filter_map(|(slot, row)| row.map(|row| (slot, row)))
                .min_by_key(|(_, row)| order(*row));
            let Some((slot, row)) = chosen else {
                break;
            };
            if last.is_some_and(|previous| previous >= order(row)) {
                return Err(IndexError::BackendViolation(
                    "duplicate literal delta across runs".into(),
                ));
            }
            last = Some(order(row));
            output.push(row);
            if output.len() == PAGE_ENTRIES {
                self.write_page(&mut target, &output, budget)?;
                output.clear();
            }
            *heads.get_mut(slot).ok_or(IndexError::ResourceLimited)? = readers
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?
                .next(self, budget)?;
        }
        if !output.is_empty() {
            self.write_page(&mut target, &output, budget)?;
        }
        for run in group {
            self.delete_run(*run, budget)?;
        }
        Ok(target)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct ResidentHead {
    row: MembershipEdit,
    slot: usize,
}

impl Ord for ResidentHead {
    fn cmp(&self, other: &Self) -> Ordering {
        (order(self.row), self.slot).cmp(&(order(other.row), other.slot))
    }
}

impl PartialOrd for ResidentHead {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) struct DeltaStream(DeltaStreamInner, bool);

enum DeltaStreamInner {
    Resident {
        chunks: Vec<ResidentChunk>,
        frontier: BinaryHeap<Reverse<ResidentHead>>,
        last: Option<([u8; 3], u32)>,
        _metadata: Option<MemoryLease>,
        _frontier_memory: Option<MemoryLease>,
    },
    Spilled {
        reader: RunReader,
        store: RunStore,
    },
}

impl DeltaStream {
    pub(super) fn next(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<MembershipEdit>, IndexError> {
        if self.1 {
            return Err(IndexError::BackendViolation(
                "literal delta stream poisoned".into(),
            ));
        }
        let result = (|| {
            budget.checkpoint()?;
            match &mut self.0 {
                DeltaStreamInner::Resident {
                    chunks,
                    frontier,
                    last,
                    ..
                } => {
                    let Some(Reverse(ResidentHead { row, slot })) = frontier.pop() else {
                        return Ok(None);
                    };
                    if last.is_some_and(|prior| prior >= order(row)) {
                        return Err(IndexError::BackendViolation(
                            "duplicate literal delta across resident chunks".into(),
                        ));
                    }
                    *last = Some(order(row));
                    let chunk = chunks.get_mut(slot).ok_or(IndexError::ResourceLimited)?;
                    if let Some(next) = chunk.rows.get(chunk.cursor).copied() {
                        frontier.push(Reverse(ResidentHead { row: next, slot }));
                    }
                    chunk.cursor = chunk
                        .cursor
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    Ok(Some(row))
                }
                DeltaStreamInner::Spilled { reader, store } => reader.next(store, budget),
            }
        })();
        if result.is_err() {
            self.1 = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use quanta_index_core::index::WorkLimits;

    use super::*;

    struct TestScratch;
    struct Allocation(tempfile::TempDir);

    impl ScratchAllocation for Allocation {
        fn path(&self) -> &Path {
            self.0.path()
        }
        fn max_bytes(&self) -> u64 {
            SCRATCH_MAX_BYTES
        }
        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(SCRATCH_MAX_BYTES)
        }
        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > SCRATCH_MAX_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            Ok(())
        }
    }

    impl ScratchSpacePort for TestScratch {
        fn allocate(
            &self,
            request: ScratchRequest,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            budget.checkpoint()?;
            if request.max_bytes != SCRATCH_MAX_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            Ok(Box::new(Allocation(tempfile::tempdir().map_err(
                |error| IndexError::Storage(error.to_string()),
            )?)))
        }
    }

    fn work() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 1_000_000_000,
                mutations: 1_000_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    fn edit(number: u32) -> MembershipEdit {
        MembershipEdit {
            ordinal: 0,
            gram: [(number >> 16) as u8, (number >> 8) as u8, number as u8],
            add: number % 2 == 0,
        }
    }

    #[test]
    fn resident_sorted_stream_refuses_duplicate_and_poisons() -> Result<(), IndexError> {
        let mut budget = work()?;
        let mut collector = DeltaCollector::new(&budget)?;
        collector.extend(&[edit(3), edit(1), edit(2)], &TestScratch, &mut budget)?;
        let mut stream = collector.finish(&TestScratch, &mut budget)?;
        for number in 1..=3 {
            if stream.next(&mut budget)? != Some(edit(number)) {
                return Err(IndexError::BackendViolation(
                    "resident literal delta order".into(),
                ));
            }
        }
        if stream.next(&mut budget)?.is_some() {
            return Err(IndexError::BackendViolation(
                "resident literal delta extra row".into(),
            ));
        }
        let mut collector = DeltaCollector::new(&budget)?;
        collector.extend(&[edit(1), edit(1)], &TestScratch, &mut budget)?;
        if collector.finish(&TestScratch, &mut budget).is_ok() {
            return Err(IndexError::BackendViolation(
                "duplicate resident delta accepted".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn binary_page_spill_merges_exact_stream() -> Result<(), IndexError> {
        let mut budget = work()?;
        let mut collector = DeltaCollector::new(&budget)?;
        // Force the production spill path at one fixed 1 MiB chunk.
        collector.max_chunks = 1;
        let count = RESIDENT_CHUNK_ENTRIES + 31;
        for start in (0..count).step_by(65_536) {
            let end = (start + 65_536).min(count);
            let chunk = (start..end)
                .rev()
                .map(|number| edit(number as u32))
                .collect::<Vec<_>>();
            collector.extend(&chunk, &TestScratch, &mut budget)?;
        }
        let mut stream = collector.finish(&TestScratch, &mut budget)?;
        for number in 0..count {
            if stream.next(&mut budget)? != Some(edit(number as u32)) {
                return Err(IndexError::BackendViolation(format!(
                    "spilled literal delta differs at {number}"
                )));
            }
        }
        if stream.next(&mut budget)?.is_some() {
            return Err(IndexError::BackendViolation(
                "spilled literal delta extra row".into(),
            ));
        }
        Ok(())
    }
}
