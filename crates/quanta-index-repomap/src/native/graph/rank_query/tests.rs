mod parity;

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_contract::{
    CompiledRepoMapProjectionEntryV1, FileId, RepoMapDocType, RepoMapFocusSubjectDto,
    RepoMapNodeRef, RepoRelativePath,
    index::{Digest, MapRoot, ObjectRef, OwnerKey, ProducerId, StableOwnerId},
    native_protocol::NativeRepoMapQuery,
};
use quanta_index_core::index::{
    IndexError, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite, MemoryLease,
    MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort, ObjectBatchInput,
    RegisteredObject, RegisteredObjectBatch, StoredObject, WorkBudget, WorkLimits,
    ports::{ImmutableObjectPort, MapEditCursor, ObjectWriteSink, PersistentMapPort},
};

use super::{query_rank, validate_request};
use crate::native::graph::{
    GraphNode,
    affected::node_key,
    contribution::{GraphElement, GraphSupport},
};

struct Objects {
    values: Mutex<BTreeMap<ObjectRef, StoredObject>>,
}

impl Objects {
    fn insert(
        &self,
        reference: ObjectRef,
        value: &impl serde::Serialize,
        children: &[ObjectRef],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let payload = serde_json::to_vec(value)?;
        let registered =
            RegisteredObject::unscoped(reference, u64::try_from(payload.len())?, children.to_vec());
        let _old = self
            .values
            .lock()
            .map_err(|_error| std::io::Error::other("test lock poisoned"))?
            .insert(reference, StoredObject::new(registered, payload, None));
        Ok(())
    }
}

impl ImmutableObjectPort for Objects {
    fn identify(
        &self,
        _: &[u8],
        _: &[ObjectRef],
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected test identify".into(),
        ))
    }
    fn put(
        &self,
        _: &[u8],
        _: &[ObjectRef],
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("unexpected test put".into()))
    }
    fn install(
        &self,
        _: &[u8],
        _: &[ObjectRef],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected test install".into(),
        ))
    }
    fn install_batch(
        &self,
        _: &[ObjectBatchInput],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected test batch install".into(),
        ))
    }
    fn get(&self, reference: ObjectRef, _: &mut WorkBudget) -> Result<StoredObject, IndexError> {
        self.values
            .lock()
            .map_err(|_error| IndexError::Storage("test lock".into()))?
            .get(&reference)
            .cloned()
            .ok_or_else(|| IndexError::CorruptObject("test object absent".into()))
    }
}

struct Maps {
    entries: Vec<(Vec<u8>, ObjectRef)>,
}

impl PersistentMapPort for Maps {
    fn lookup_object(
        &self,
        _: MapRoot,
        _: &[u8],
        _: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected test lookup".into(),
        ))
    }
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
        self.lookup_object(root, key, budget)
            .map(|value| value.map(Into::into))
    }

    fn scan(
        &self,
        _: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        _: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        let rows: Vec<_> = self
            .entries
            .iter()
            .filter(|(key, _)| after.is_none_or(|prior| key.as_slice() > prior))
            .collect();
        let take = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        let has_more = rows.len() > take;
        Ok(MapPage::new(
            rows.into_iter()
                .take(take)
                .map(|(key, value)| (key.clone(), (*value).into()))
                .collect(),
            has_more,
            Vec::new(),
            None,
        ))
    }
    fn apply(
        &self,
        _: MapRoot,
        _: &[MapEdit],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        Err(IndexError::BackendViolation("unexpected test apply".into()))
    }
    fn apply_batch(
        &self,
        _: &[MapBatchRequest],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected test batch apply".into(),
        ))
    }
    fn apply_stream(
        &self,
        _: &[MapRoot],
        _: &mut dyn MapEditCursor,
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected test stream apply".into(),
        ))
    }
}

fn reference(byte: u8) -> ObjectRef {
    ObjectRef(Digest([byte; 32]))
}

fn owner(byte: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([1; 16]),
        stable_id: StableOwnerId([byte; 16]),
    }
}

fn file(value: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(value))
}

fn budget() -> Result<WorkBudget, Box<dyn std::error::Error>> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or("test deadline overflow")?;
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 0,
            bytes: 64 << 20,
            mutations: 0,
        },
        deadline,
    ))
}

fn fixture() -> Result<(Maps, Objects), Box<dyn std::error::Error>> {
    let objects = Objects {
        values: Mutex::new(BTreeMap::new()),
    };
    let mut entries = Vec::new();
    // B has the strongest score and a query hit, but belongs to a different
    // owner path. A and A2 share the focused path; the Unicode fold hits A.
    for (idx, (identity, path, text, score, owner_id)) in [
        ("a", "src/a.rs", "İstanbul", 300, 1),
        ("a2", "src/a.rs", "other", 900, 2),
        ("b", "src/b.rs", "ice", 1000, 3),
    ]
    .into_iter()
    .enumerate()
    {
        let subject = file(identity);
        let row_number = idx
            .checked_mul(2)
            .and_then(|value| value.checked_add(1))
            .ok_or("row reference overflow")?;
        let support_number = idx
            .checked_mul(2)
            .and_then(|value| value.checked_add(2))
            .ok_or("support reference overflow")?;
        let row_ref = reference(u8::try_from(row_number)?);
        let support_ref = reference(u8::try_from(support_number)?);
        let row = CompiledRepoMapProjectionEntryV1 {
            subject: subject.clone(),
            doc_type: RepoMapDocType::File,
            owner_path: RepoRelativePath::new(path),
            search_text: text.to_string(),
            final_score_millis: score,
        };
        objects.insert(row_ref, &row, &[])?;
        let support = GraphSupport {
            owner: owner(owner_id),
            element: GraphElement::Node(GraphNode {
                identity: subject.clone(),
                payload_digest: Digest([7; 32]),
            }),
            projection: Some(row_ref),
        };
        objects.insert(support_ref, &support, &[row_ref])?;
        entries.push((node_key(&subject, owner(owner_id))?, support_ref));
    }
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok((Maps { entries }, objects))
}

fn query(focus: Vec<RepoMapFocusSubjectDto>) -> NativeRepoMapQuery {
    NativeRepoMapQuery {
        query_text: "i".into(),
        focus_subjects: focus,
        top_k: 3,
        token_budget: 1000,
    }
}

fn subjects(page: &super::NativeRepoMapRankPage) -> Vec<&str> {
    page.hits()
        .iter()
        .map(|hit| match &hit.subject {
            RepoMapNodeRef::File(id) => id.as_str(),
            RepoMapNodeRef::Module(id) => id.as_str(),
            RepoMapNodeRef::Symbol(id) => id.as_str(),
            RepoMapNodeRef::Chunk(id) => id.as_str(),
        })
        .collect()
}

#[test]
fn selected_view_rank_focus_excludes_stronger_unrelated_owner_and_preserves_fold()
-> Result<(), Box<dyn std::error::Error>> {
    let (maps, objects) = fixture()?;
    let focused = query(vec![RepoMapFocusSubjectDto {
        subject_identity: "a".into(),
        subject_doc_type: RepoMapDocType::File,
    }]);
    validate_request(&focused)?;
    let mut focused_budget = budget()?;
    let focused_page = query_rank(
        MapRoot(Some(reference(100))),
        &focused,
        &maps,
        &objects,
        &mut focused_budget,
    )?;
    if subjects(&focused_page) != ["a", "a2"] || focused_page.dropped_entries_count != 0 {
        return Err(format!(
            "focus universe/rank differed: {:?}",
            subjects(&focused_page)
        )
        .into());
    }
    let mut broad_budget = budget()?;
    let broad_page = query_rank(
        MapRoot(Some(reference(100))),
        &query(Vec::new()),
        &maps,
        &objects,
        &mut broad_budget,
    )?;
    if subjects(&broad_page) != ["b", "a", "a2"] {
        return Err(format!("folded broad ranking differed: {:?}", subjects(&broad_page)).into());
    }
    let mut low_budget = query(Vec::new());
    low_budget.token_budget = 1;
    let low_page = query_rank(
        MapRoot(Some(reference(100))),
        &low_budget,
        &maps,
        &objects,
        &mut budget()?,
    )?;
    if subjects(&low_page) != ["b"]
        || low_page.dropped_entries_count != 2
        || low_page.drop_reason_codes() != ["token_budget_exhausted"]
        || low_page.degraded_reason_codes() != ["token_budget_floor_applied"]
    {
        return Err("rank token floor/drop contract changed".into());
    }
    let missing = query(vec![RepoMapFocusSubjectDto {
        subject_identity: "missing".into(),
        subject_doc_type: RepoMapDocType::File,
    }]);
    let mut missing_budget = budget()?;
    if !matches!(
        query_rank(
            MapRoot(Some(reference(100))),
            &missing,
            &maps,
            &objects,
            &mut missing_budget
        ),
        Err(IndexError::FocusSubjectNotFound)
    ) {
        return Err("missing focus was not refused".into());
    }
    Ok(())
}

struct PageMemoryCount(AtomicU64);

impl MemoryLeaseOwner for PageMemoryCount {
    fn release(&self, bytes: u64) {
        let previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        assert!(previous >= bytes, "rank page released excess charge");
    }
}

struct PageMemoryScope(Arc<PageMemoryCount>);

impl MemoryScopePort for PageMemoryScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self.0.0.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

#[test]
fn rank_page_releases_source_text_and_keeps_result_charge() -> Result<(), Box<dyn std::error::Error>>
{
    let (maps, objects) = fixture()?;
    let large = CompiledRepoMapProjectionEntryV1 {
        subject: file("a"),
        doc_type: RepoMapDocType::File,
        owner_path: RepoRelativePath::new("src/a.rs"),
        search_text: "i".repeat(512 << 10),
        final_score_millis: 300,
    };
    objects.insert(reference(1), &large, &[])?;
    let count = Arc::new(PageMemoryCount(AtomicU64::new(0)));
    let mut work = budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(PageMemoryScope(
        Arc::clone(&count),
    )))));
    let page = query_rank(
        MapRoot(Some(reference(100))),
        &query(Vec::new()),
        &maps,
        &objects,
        &mut work,
    )?;
    let retained = count.0.load(Ordering::SeqCst);
    // Three short identities/paths and diagnostics must not retain a half-MiB
    // preview, its decoded backing or its folded search copy after ranking.
    if page.hits().len() != 3 || retained == 0 || retained >= 128 << 10 {
        return Err(format!("rank result retained source text or lost custody: {retained}").into());
    }
    drop(page);
    if count.0.load(Ordering::SeqCst) != 0 {
        return Err("rank result leaked retained charge".into());
    }
    Ok(())
}
