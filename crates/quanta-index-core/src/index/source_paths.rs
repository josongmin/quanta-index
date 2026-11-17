//! Read repository members of one path in the selected SourceCut.

use quanta_index_contract::RepoId;

use super::{
    IndexError, MapPageEntry, MapRoot, MapValue, MemoryLifetime, ObjectRef, OwnerKey,
    SourcePathOwners, SourcePathRepositoryKey, SourceRecordOwner, WorkBudget, WorkUsage,
    owner_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

/// Validate a repository bucket key, including the original unscoped slot.
pub fn validate_source_path_repository_key(key: &[u8]) -> Result<(), IndexError> {
    let suffix = key
        .strip_prefix(b"repo\0")
        .ok_or_else(|| IndexError::CorruptObject("repository path map has a foreign key".into()))?;
    if suffix.is_empty() {
        return Ok(());
    }
    let name = std::str::from_utf8(suffix)
        .map_err(|_error| IndexError::CorruptObject("repository path key is not UTF-8".into()))?;
    RepoId::validate(name)
        .map_err(|error| IndexError::CorruptObject(format!("repository path key: {error}")))
}

/// Visit every member of one nonempty repository bucket in key order. A caller
/// must discard any accumulated output when a page or member is invalid.
pub fn visit_repository_path_owners(
    root: ObjectRef,
    maps: &dyn PersistentMapPort,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&[u8], MapValue, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    const PAGE_ROWS: u32 = 128;
    let mut after: Option<MapPageEntry> = None;
    loop {
        budget.checkpoint()?;
        let mut previous = after.as_ref().map(MapPageEntry::key).transpose()?;
        let page = maps.scan(MapRoot(Some(root)), previous, PAGE_ROWS, budget)?;
        if page.entries.is_empty() {
            return Err(if after.is_some() || page.has_more {
                IndexError::BackendViolation("repository path scan did not advance".into())
            } else {
                IndexError::CorruptObject("source path repository bucket is empty".into())
            });
        }
        if page.entries.len()
            > usize::try_from(PAGE_ROWS).map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::BackendViolation(
                "repository path scan escaped its ordered page".into(),
            ));
        }
        for (key, value) in &page.entries {
            let key = key.as_slice();
            if previous.is_some_and(|prior| key <= prior) {
                return Err(IndexError::BackendViolation(
                    "repository path scan repeated or reversed a key".into(),
                ));
            }
            validate_source_path_repository_key(key)?;
            visit(key, *value, budget)?;
            previous = Some(key);
        }
        if !page.has_more {
            return budget.checkpoint();
        }
        after = Some(page.cursor().last().ok_or_else(|| {
            IndexError::BackendViolation("repository path scan lost its continuation".into())
        })?);
    }
}

/// Resolve bucket children to source records under the same cut. Each child
/// must match both the queried path and its repository key. Decoded records
/// carry retained memory custody into the caller.
pub fn visit_repository_path_sources(
    root: ObjectRef,
    path: &str,
    owners: MapRoot,
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(
        OwnerKey,
        ObjectRef,
        SourceRecordOwner,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    visit_repository_path_owners(root, maps, budget, |key, marker, budget| {
        let SourcePathOwners::LegacyOwner(owner) = SourcePathOwners::decode(marker, budget)? else {
            return Err(IndexError::CorruptObject(
                "repository path owner must not have another child map".into(),
            ));
        };
        let reference = maps
            .lookup_object(owners, &owner_map_key(owner), budget)?
            .ok_or_else(|| {
                IndexError::CorruptObject("repository path lacks source owner".into())
            })?;
        let stored = objects.get(reference, budget)?;
        let record =
            SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Retained)?;
        let record_key = SourcePathRepositoryKey::new(record.source_repo_id.as_ref())?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                path.len()
                    .checked_add(key.len())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if stored.registration.reference != reference
            || stored.registration.children.as_slice() != [record.bytes]
            || record.path != path
            || record_key.as_slice()? != key
        {
            return Err(IndexError::CorruptObject(
                "repository path differs from selected source record".into(),
            ));
        }
        visit(owner, reference, record, budget)
    })
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_contract::index::{
        Digest, ProducerId, SourceRecord, SourceTextAdmission, StableOwnerId,
    };

    use super::super::{
        MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite, ObjectBatchInput,
        RegisteredObject, StagedMapRoots, StoredObject, WorkLimits,
        ports::{MapEditCursor, ObjectWriteSink},
    };
    use super::*;

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }
    fn owner(byte: u8) -> OwnerKey {
        OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([byte; 16]),
        }
    }
    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 0,
                bytes: 100_000,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }
    fn page(rows: Vec<(Vec<u8>, MapValue)>, more: bool) -> MapPage {
        MapPage::new(rows, more, Vec::new(), None)
    }
    fn row(key: &[u8], value: SourcePathOwners) -> Result<(Vec<u8>, MapValue), IndexError> {
        Ok((key.to_vec(), value.encode()?))
    }

    struct Maps {
        pages: Vec<MapPage>,
        scans: AtomicUsize,
        owners: Vec<(OwnerKey, ObjectRef)>,
    }
    impl PersistentMapPort for Maps {
        fn lookup(
            &self,
            _root: MapRoot,
            key: &[u8],
            _budget: &mut WorkBudget,
        ) -> Result<Option<MapValue>, IndexError> {
            Ok(self
                .owners
                .iter()
                .find(|(owner, _)| owner_map_key(*owner) == key)
                .map(|(_, reference)| (*reference).into()))
        }
        fn scan(
            &self,
            _root: MapRoot,
            _after: Option<&[u8]>,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            self.pages
                .get(self.scans.fetch_add(1, Ordering::Relaxed))
                .cloned()
                .ok_or_else(|| IndexError::BackendViolation("unexpected scan".into()))
        }
        fn apply(
            &self,
            _base: MapRoot,
            _edits: &[MapEdit],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation("unexpected apply".into()))
        }
        fn apply_batch(
            &self,
            _requests: &[MapBatchRequest],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected apply batch".into(),
            ))
        }
        fn apply_stream(
            &self,
            _bases: &[MapRoot],
            _edits: &mut dyn MapEditCursor,
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected apply stream".into(),
            ))
        }
    }

    struct Objects(Vec<(ObjectRef, StoredObject)>);
    impl ImmutableObjectPort for Objects {
        fn identify(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected identify".into()))
        }
        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected put".into()))
        }
        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected install".into()))
        }
        fn install_batch(
            &self,
            _inputs: &[ObjectBatchInput],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<super::super::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected install batch".into(),
            ))
        }
        fn get(
            &self,
            reference: ObjectRef,
            _budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            self.0
                .iter()
                .find(|(key, _)| *key == reference)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| IndexError::CorruptObject("source record missing".into()))
        }
    }
    fn stored(record_ref: ObjectRef, repo: Option<&str>) -> Result<StoredObject, IndexError> {
        let record = SourceRecord {
            path: "src/lib.rs".into(),
            bytes: reference(99),
            byte_len: 0,
            text: SourceTextAdmission::Excluded,
            source_repo_id: repo
                .map(RepoId::try_from)
                .transpose()
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?,
        };
        let payload = serde_json::to_vec(&record)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        Ok(StoredObject::new(
            RegisteredObject::unscoped(record_ref, payload.len() as u64, vec![record.bytes]),
            payload,
            None,
        ))
    }

    #[test]
    fn repository_members_include_legacy_and_distinct_repositories() -> Result<(), IndexError> {
        let members = [
            (b"repo\0".as_slice(), 1, None),
            (b"repo\0a", 2, Some("a")),
            (b"repo\0b", 3, Some("b")),
        ];
        let rows = members
            .iter()
            .map(|(key, ordinal, _)| row(key, SourcePathOwners::LegacyOwner(owner(*ordinal))))
            .collect::<Result<Vec<_>, _>>()?;
        let maps = Maps {
            pages: vec![page(rows, false)],
            scans: AtomicUsize::new(0),
            owners: members
                .iter()
                .map(|(_, ordinal, _)| (owner(*ordinal), reference(*ordinal)))
                .collect(),
        };
        let objects = Objects(
            members
                .iter()
                .map(|(_, ordinal, repo)| {
                    stored(reference(*ordinal), *repo).map(|value| (reference(*ordinal), value))
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let mut observed = Vec::new();
        visit_repository_path_sources(
            reference(10),
            "src/lib.rs",
            MapRoot(Some(reference(11))),
            &maps,
            &objects,
            &mut budget()?,
            &mut |owner, _, record, _| {
                observed.push((
                    owner,
                    record
                        .source_repo_id
                        .as_ref()
                        .map(RepoId::as_str)
                        .map(str::to_owned),
                ));
                Ok(())
            },
        )?;
        if observed
            != vec![
                (owner(1), None),
                (owner(2), Some("a".into())),
                (owner(3), Some("b".into())),
            ]
        {
            return Err(IndexError::BackendViolation(
                "repository members differed".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn repository_bucket_refuses_mismatched_and_malformed_members() -> Result<(), IndexError> {
        let bucket = reference(10);
        let wrong = Maps {
            pages: vec![page(
                vec![row(b"repo\0a", SourcePathOwners::LegacyOwner(owner(1)))?],
                false,
            )],
            scans: AtomicUsize::new(0),
            owners: vec![(owner(1), reference(1))],
        };
        let objects = Objects(vec![(reference(1), stored(reference(1), Some("b"))?)]);
        if !matches!(
            visit_repository_path_sources(
                bucket,
                "src/lib.rs",
                MapRoot(Some(reference(11))),
                &wrong,
                &objects,
                &mut budget()?,
                &mut |_, _, _, _| Ok(())
            ),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err(IndexError::BackendViolation(
                "wrong repository record accepted".into(),
            ));
        }
        let cases = vec![
            vec![page(
                vec![row(
                    b"repo\0a",
                    SourcePathOwners::Repositories(reference(12)),
                )?],
                false,
            )],
            vec![page(Vec::new(), false)],
            vec![
                page(
                    vec![row(b"repo\0b", SourcePathOwners::LegacyOwner(owner(2)))?],
                    true,
                ),
                page(
                    vec![row(b"repo\0a", SourcePathOwners::LegacyOwner(owner(1)))?],
                    false,
                ),
            ],
            vec![
                page(
                    vec![row(b"repo\0a", SourcePathOwners::LegacyOwner(owner(1)))?],
                    true,
                ),
                page(Vec::new(), false),
            ],
            vec![page(
                vec![row(b"foreign", SourcePathOwners::LegacyOwner(owner(1)))?],
                false,
            )],
        ];
        for pages in cases {
            let maps = Maps {
                pages,
                scans: AtomicUsize::new(0),
                owners: Vec::new(),
            };
            let result = if maps.pages.len() == 1 && !maps.pages[0].entries.is_empty() {
                visit_repository_path_sources(
                    bucket,
                    "src/lib.rs",
                    MapRoot(None),
                    &maps,
                    &objects,
                    &mut budget()?,
                    &mut |_, _, _, _| Ok(()),
                )
            } else {
                visit_repository_path_owners(bucket, &maps, &mut budget()?, |_, _, _| Ok(()))
            };
            if !matches!(
                result,
                Err(IndexError::CorruptObject(_) | IndexError::BackendViolation(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "malformed bucket accepted".into(),
                ));
            }
        }
        Ok(())
    }
}
