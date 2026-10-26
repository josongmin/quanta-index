//! Existing ingress and immutable map remain the sole staging authority.
use super::*;
use quanta_index_core::index::{MapRoot, MapValue, SourceTextAdmission};

#[test]
fn inline_mutation_pages_seal_original_digest_and_reopen_without_expected_edges() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let _genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let sources = engine.stage_sources(
        ingress,
        &[SourceInput {
            path: "same-source.rs".into(),
            bytes: b"source body".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &mut budget(),
    )?;
    let next = *sources.first().ok_or("missing next source")?;
    // Deliberately absent from physical storage: a historical CAS stamp is not
    // a GC edge and must not need a registered object to stage or replay.
    let historical = ObjectRef(Digest([213; 32]));
    let rows: Vec<_> = (0..257)
        .map(|id| OwnerMutation {
            owner: large_owner(id),
            expected: if id % 2 == 0 { None } else { Some(historical) },
            next: if id % 3 == 0 { None } else { Some(next) },
        })
        .collect();
    let mut hash = Sha256::new();
    hash.update(b"quanta-index-owner-mutation-stream-v1\0");
    for row in &rows {
        let payload = serde_json::to_vec(row)?;
        hash.update(owner_map_key(row.owner));
        hash.update(u64::try_from(payload.len())?.to_be_bytes());
        hash.update(payload);
    }
    hash.update(257_u64.to_be_bytes());
    let digest = Digest(hash.finalize().into());
    let mut root = MapRoot(None);
    for page in rows.chunks(128) {
        root = engine.stage_mutation_page(ingress, root, page, &mut budget())?;
    }
    assert!(matches!(
        engine.seal_mutations(ingress, root, 256, &mut budget()),
        Err(IndexError::InvalidInput(_))
    ));
    let sealed = engine.seal_mutations(ingress, root, 257, &mut budget())?;
    assert_eq!(sealed.digest, digest);
    let mut after = None;
    let mut observed = 0usize;
    loop {
        let page = objects.scan(root, after.as_deref(), 128, &mut budget())?;
        for (key, value) in &page.entries {
            let row = rows.get(observed).ok_or("extra mutation map row")?;
            assert_eq!(key.as_slice(), owner_map_key(row.owner));
            let MapValue::Inline(value) = value else {
                return Err("mutation wrapper persisted".into());
            };
            assert_eq!(
                value.data()?,
                row.expected
                    .as_ref()
                    .map_or(&[][..], |reference| reference.0.0.as_slice())
            );
            assert_eq!(value.children()?, row.next.as_slice());
            after = Some(key.clone());
            observed = observed.checked_add(1).ok_or("row count overflow")?;
        }
        if !page.has_more {
            break;
        }
    }
    assert_eq!(observed, rows.len());
    let streamed = engine.stage_mutations(ingress, rows.into_iter().map(Ok), &mut budget())?;
    assert_eq!(streamed.count, sealed.count);
    assert_eq!(streamed.digest, sealed.digest);
    drop(engine);
    drop(objects);
    drop(catalog);
    let (reopened, _objects, _catalog) = kernel(directory.path())?;
    assert_eq!(
        reopened.seal_mutations(ingress, root, 257, &mut budget())?,
        sealed
    );
    Ok(())
}
