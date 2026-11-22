use super::*;
use quanta_index_core::index::{
    OwnerKey, ProducerId, RepoId, SourceCut, SourcePathKey, SourcePathOwners,
    SourcePathRepositoryKey, StableOwnerId, owner_map_key,
};

#[test]
fn held_source_reader_preserves_same_path_foreign_repositories_and_binary_bytes() -> TestResult {
    let root = private_root()?;
    let store = FileObjectStore::open(root.path())?;
    let mut work = budget(500)?;
    let repositories = [RepoId::new("first")?, RepoId::new("foreign")?];
    let contents = [vec![0, 255, 1], b"old complete text".to_vec()];
    let mut owner_edits = Vec::new();
    let mut repo_edits = Vec::new();
    for (ordinal, (repository, bytes)) in repositories.iter().zip(&contents).enumerate() {
        let body = store.put(bytes, &[], &mut work)?.reference;
        let digest = body
            .0
            .0
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let encoded = format!(
            "{{\"path\":\"shared.rs\",\"bytes\":[{digest}],\"byte_len\":{},\"text\":\"Excluded\",\"source_repo_id\":\"{}\"}}",
            bytes.len(),
            repository.as_str(),
        );
        let record = store.put(encoded.as_bytes(), &[body], &mut work)?.reference;
        let owner = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([ordinal as u8 + 1; 16]),
        };
        owner_edits.push(MapEdit::objects(
            owner_map_key(owner).to_vec(),
            None,
            Some(record),
        ));
        repo_edits.push(MapEdit {
            key: SourcePathRepositoryKey::new(Some(repository))?
                .as_slice()?
                .to_vec(),
            expected: None,
            next: Some(SourcePathOwners::LegacyOwner(owner).encode()?),
        });
    }
    // The canonical map requires ordered edits; repo identity is independent
    // of the containing revision and the shared path itself.
    repo_edits.sort_unstable_by(|left, right| left.key.cmp(&right.key));
    let owners = store
        .apply(MapRoot(None), &owner_edits, &AcceptMapInstall, &mut work)?
        .root;
    let repositories_root = store
        .apply(MapRoot(None), &repo_edits, &AcceptMapInstall, &mut work)?
        .root
        .0
        .ok_or("repository bucket absent")?;
    let paths = store
        .apply(
            MapRoot(None),
            &[MapEdit {
                key: SourcePathKey::copy("shared.rs", &work)?.as_slice().to_vec(),
                expected: None,
                next: Some(SourcePathOwners::Repositories(repositories_root).encode()?),
            }],
            &AcceptMapInstall,
            &mut work,
        )?
        .root;
    let cut = SourceCut {
        owners,
        paths,
        frontiers: MapRoot(None),
    };
    drop(store);
    let reads = Arc::new(Reads::default());
    let held = reader(root.path(), reads)?;
    let mut work = budget(500)?;
    require(held.source_file_count_v1(&cut, &mut work)? == 2)?;
    for (repository, expected) in repositories.iter().zip(&contents) {
        held.with_repository_source_file_v1(
            &cut,
            repository,
            "shared.rs",
            &mut work,
            |_, record, bytes, _| {
                require(record.source_repo_id.as_ref() == Some(repository))?;
                require(bytes == expected)?;
                Ok(())
            },
        )?;
    }
    require(matches!(
        held.with_repository_source_file_v1(
            &cut,
            &RepoId::new("absent")?,
            "shared.rs",
            &mut work,
            |_, _, _, _| Ok(()),
        ),
        Err(IndexError::SnapshotUnavailable)
    ))?;
    require(work.usage().writes == 0)?;
    Ok(())
}
