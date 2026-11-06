use super::super::super::batch_metadata::BatchShards;
use super::*;
use std::path::Path;

fn registration(first: u8) -> RegisteredObject {
    RegisteredObject::unscoped(ObjectRef(Digest([first; 32])), 0, Vec::new())
}

#[test]
fn batch_shards_are_unique_sorted_and_refund_path_backing() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let objects = directory.path().join("objects");
    let registrations = [
        registration(255),
        registration(16),
        registration(0),
        registration(15),
        registration(255),
        registration(0),
    ];
    let (work, ledger) = budget(10_000)?;
    let shards = BatchShards::new(&objects, &registrations, &work)?;
    let actual: Vec<_> = shards.iter().map(Path::to_path_buf).collect();
    check!(
        actual
            == ["00", "0f", "10", "ff"]
                .map(|suffix| objects.join(suffix))
                .to_vec()
    );
    check!(ledger.live.load(Ordering::SeqCst) > 0);
    drop(shards);
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    Ok(())
}

#[test]
fn batch_shard_memory_denial_precedes_path_allocation() -> Result<(), Box<dyn std::error::Error>> {
    let objects = Path::new("/fixed/objects");
    let (work, ledger) = budget(0)?;
    check!(matches!(
        BatchShards::new(objects, &[registration(16)], &work),
        Err(IndexError::ResourceLimited)
    ));
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    check!(ledger.peak.load(Ordering::SeqCst) == 0);
    Ok(())
}

fn input(slot: u16) -> ObjectBatchInput {
    ObjectBatchInput {
        payload: slot.to_be_bytes().to_vec(),
        children: Vec::new(),
    }
}

#[test]
fn public_65_and_256_install_one_packet_with_original_output_order()
-> Result<(), Box<dyn std::error::Error>> {
    for count in [65_usize, 256] {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let (mut work, ledger) = budget(2_000_000)?;
        let sink = sink(&ledger, false);
        let mut inputs = Vec::new();
        for ordinal in 0..count {
            let slot = u16::try_from(ordinal)?;
            inputs.push(input(slot));
        }
        // The 65-input case crosses the former bound and proves that a
        // duplicate does not compact the returned ordinal sequence.
        if count == 65 {
            let duplicate = count.checked_sub(2).ok_or("missing duplicate slot")?;
            *inputs.get_mut(duplicate).ok_or("missing duplicate slot")? = input(7);
        }
        let outputs = store.install_batch(&inputs, &sink, &mut work)?;
        check!(outputs.len() == count);
        check!(sink.reserve.load(Ordering::SeqCst) == 1);
        check!(sink.confirm.load(Ordering::SeqCst) == 1);
        if count == 65 {
            let duplicate = count.checked_sub(2).ok_or("missing duplicate slot")?;
            check!(
                outputs
                    .get(duplicate)
                    .ok_or("missing duplicate output")?
                    .reference
                    == outputs.get(7).ok_or("missing original output")?.reference
            );
        }
        for ordinal in 0..count {
            let expected = inputs.get(ordinal).ok_or("missing fixed input")?;
            let actual = outputs.get(ordinal).ok_or("missing ordered output")?;
            let stored = store.get(actual.reference, &mut work)?;
            check!(stored.payload == expected.payload);
            check!(stored.registration.children.is_empty());
        }
        drop(outputs);
        check!(ledger.live.load(Ordering::SeqCst) == 0);
    }
    Ok(())
}

#[test]
fn public_257_refuses_before_memory_custody_or_file_io() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store_path = directory.path().join("store");
    let observer = Arc::new(ObjectIoObservation::default());
    let store = FileObjectStore::open(&store_path)?.with_io_observation(observer.clone());
    let (mut work, ledger) = budget(2_000_000)?;
    let sink = sink(&ledger, false);
    let inputs: Vec<_> = (0_u16..257).map(input).collect();
    let peak_before = ledger.peak.load(Ordering::SeqCst);
    check!(matches!(
        store.install_batch(&inputs, &sink, &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check!(ledger.peak.load(Ordering::SeqCst) == peak_before);
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    check!(sink.reserve.load(Ordering::SeqCst) == 0);
    check!(sink.confirm.load(Ordering::SeqCst) == 0);
    let sample = observer.snapshot()?;
    check!(sample.reserve_calls == 0 && sample.write_calls == 0);
    check!(std::fs::read_dir(store_path.join("objects"))?.count() == 0);
    Ok(())
}
