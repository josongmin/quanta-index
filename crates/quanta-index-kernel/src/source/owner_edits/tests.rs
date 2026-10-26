use std::error::Error;

use quanta_index_core::index::{
    MapEdit, MapRoot, OwnerKey, ProducerId, RegisteredObject, StableOwnerId,
    ports::{ImmutableObjectPort, ObjectWriteSink, PersistentMapPort},
};
use quanta_index_storage::FileObjectStore;

use super::*;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct Sink;
impl ObjectWriteSink for Sink {
    fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }
    fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }
    fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
    fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
}

struct Fixture {
    paths: PathScratch,
    objects: FileObjectStore,
    set: OwnerMutationSet,
    mutations: Vec<OwnerMutation>,
    _home: tempfile::TempDir,
}

fn fixture() -> TestResult<Fixture> {
    let (home, paths) = super::super::tests::path_fixture()?;
    let objects = FileObjectStore::open(home.path().join("objects"))?;
    let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    let value = objects.put(b"next", &[], &mut work)?.reference;
    let mut edits = Vec::new();
    let mut mutations = Vec::new();
    let mut digest = OwnerMutationDigest::default();
    for stable in [0, 128, 255] {
        let mutation = OwnerMutation {
            owner: OwnerKey {
                producer: ProducerId([255; 16]),
                stable_id: StableOwnerId([stable; 16]),
            },
            expected: Some(ObjectRef(Digest([255; 32]))),
            next: Some(value),
        };
        let bytes = serde_json::to_vec(&mutation)?;
        if bytes.len() > MAX_MUTATION_JSON {
            return Err("fixed mutation exceeds JSON bound".into());
        }
        let stored = objects.put(&bytes, &mutation.object_children(), &mut work)?;
        digest.push(&mutation)?;
        edits.push(MapEdit {
            key: owner_map_key(mutation.owner).to_vec(),
            expected: None,
            next: Some((stored.reference).into()),
        });
        mutations.push(mutation);
    }
    let root = objects.apply(MapRoot(None), &edits, &Sink, &mut work)?.root;
    Ok(Fixture {
        paths,
        objects,
        set: OwnerMutationSet {
            root,
            count: digest.count(),
            digest: digest.finish(),
        },
        mutations,
        _home: home,
    })
}

fn spool<'a>(fixture: &'a Fixture, work: &mut WorkBudget) -> TestResult<OwnerEditCursor<'a>> {
    let mut original = OwnerMutationCursor::new(fixture.set, &fixture.objects, &fixture.objects);
    fixture.paths.begin_owner_spool(work)?;
    while let Some(mutation) = original.next_mutation(work)? {
        fixture.paths.append_owner_edit(&mutation, work)?;
    }
    Ok(fixture
        .paths
        .validated_owner_edits(&original, fixture.set, work)?)
}

#[test]
fn complete_owner_spool_matches_original_and_preserves_refusal_position() -> TestResult {
    let fixture = fixture()?;
    let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    let mut cursor = spool(&fixture, &mut work)?;
    let before = work.usage().reads;
    let first = cursor
        .next_mutation(&mut work)?
        .ok_or("first owner edit missing")?;
    let (mut denied, denied_ledger) = crate::path_staging::tests::budget(0)?;
    if !matches!(
        cursor.next_mutation(&mut denied),
        Err(IndexError::ResourceLimited)
    ) {
        return Err("encoding allocation must refuse before cursor advancement".into());
    }
    if denied_ledger.live() != 0 {
        return Err("refused encoding leaked memory".into());
    }
    let mut actual = vec![first];
    while let Some(mutation) = cursor.next_mutation(&mut work)? {
        actual.push(mutation);
    }
    if actual != fixture.mutations || work.usage().reads - before != 3 {
        return Err("spool changed canonical stream or read allowance".into());
    }
    if cursor.next_mutation(&mut work)?.is_some() || work.usage().reads - before != 3 {
        return Err("completed owner cursor read again".into());
    }
    Ok(())
}

#[test]
fn owner_spool_requires_original_completion_before_commit() -> TestResult {
    let fixture = fixture()?;
    let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    let mut original = OwnerMutationCursor::new(fixture.set, &fixture.objects, &fixture.objects);
    fixture.paths.begin_owner_spool(&work)?;
    if !matches!(
        fixture
            .paths
            .validated_owner_edits(&original, fixture.set, &mut work),
        Err(IndexError::BackendViolation(_))
    ) {
        return Err("unfinished manifest produced replay authority".into());
    }
    while let Some(mutation) = original.next_mutation(&mut work)? {
        fixture.paths.append_owner_edit(&mutation, &mut work)?;
    }
    let mut wrong = fixture.set;
    wrong.count += 1;
    if !matches!(
        fixture
            .paths
            .validated_owner_edits(&original, wrong, &mut work),
        Err(IndexError::BackendViolation(_))
    ) {
        return Err("foreign manifest produced replay authority".into());
    }
    let mut cursor = fixture
        .paths
        .validated_owner_edits(&original, fixture.set, &mut work)?;
    while cursor.next_mutation(&mut work)?.is_some() {}
    Ok(())
}

#[test]
fn owner_spool_rejects_missing_changed_extra_and_malformed_rows() -> TestResult {
    for fault in [
        "missing",
        "changed",
        "extra",
        "shape",
        "namespace",
        "extended",
        "non_blob",
    ] {
        let fixture = fixture()?;
        let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
        let mut cursor = spool(&fixture, &mut work)?;
        let mutation = fixture
            .mutations
            .first()
            .ok_or("original mutation missing")?;
        let key = owner_map_key(mutation.owner);
        let db = fixture.paths.connection.scoped()?;
        let changed = match fault {
            "missing" => db.execute(
                "DELETE FROM owner_edits WHERE owner=?1",
                params![key.as_slice()],
            )?,
            "changed" => db.execute(
                "UPDATE owner_edits SET next=NULL WHERE owner=?1",
                params![key.as_slice()],
            )?,
            "extra" => {
                let extra = owner_map_key(OwnerKey {
                    producer: ProducerId([255; 16]),
                    stable_id: StableOwnerId([254; 16]),
                });
                db.execute(
                    "INSERT INTO owner_edits(owner) VALUES(?1)",
                    params![extra.as_slice()],
                )?
            }
            "non_blob" => db.execute("INSERT INTO owner_edits(owner) VALUES('foreign')", [])?,
            "shape" => db.execute(
                "UPDATE owner_edits SET next=zeroblob(4096) WHERE owner=?1",
                params![key.as_slice()],
            )?,
            "namespace" => db.execute(
                "UPDATE owner_edits SET owner=zeroblob(38) WHERE owner=?1",
                params![key.as_slice()],
            )?,
            "extended" => {
                let mut extra = key.to_vec();
                extra.push(0);
                db.execute("INSERT INTO owner_edits(owner) VALUES(?1)", params![extra])?
            }
            _ => return Err("unknown owner spool fault".into()),
        };
        if changed != 1 {
            return Err("owner spool fault did not affect one row".into());
        }
        drop(db);
        let mut refused = false;
        loop {
            match cursor.next_mutation(&mut work) {
                Ok(Some(_)) => {}
                Err(IndexError::CorruptObject(_)) => {
                    refused = true;
                    break;
                }
                Ok(None) => break,
                Err(error) => return Err(error.into()),
            }
        }
        if !refused {
            return Err(format!("owner spool accepted {fault}").into());
        }
    }
    Ok(())
}

#[test]
fn empty_owner_spool_needs_no_retained_page_allocation() -> TestResult {
    let fixture = fixture()?;
    let (mut preparing, _preparing_ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    let empty = OwnerMutationSet {
        root: MapRoot(None),
        count: 0,
        digest: OwnerMutationDigest::default().finish(),
    };
    let mut original = OwnerMutationCursor::new(empty, &fixture.objects, &fixture.objects);
    if original.next_mutation(&mut preparing)?.is_some() {
        return Err("empty manifest returned owner".into());
    }
    // The original map scan admits its own shared page header, even when empty.
    // Only the new replay page is required to fit the zero-capacity scope.
    let (mut work, ledger) = crate::path_staging::tests::budget(0)?;
    fixture.paths.begin_owner_spool(&work)?;
    let mut cursor = fixture
        .paths
        .validated_owner_edits(&original, empty, &mut work)?;
    if cursor.next_mutation(&mut work)?.is_some() || ledger.live() != 0 {
        return Err("empty replay allocated a retained page or returned a row".into());
    }
    Ok(())
}
