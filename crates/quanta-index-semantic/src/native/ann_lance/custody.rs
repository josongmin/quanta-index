//! Keep the original Arrow staging charge alive in every buffer that Lance
//! may move into a detached source task during table creation.

use arrow_array::{Array as _, RecordBatch, RecordBatchOptions, make_array};
use arrow_buffer::{BooleanBuffer, Buffer, NullBuffer};
use arrow_data::ArrayData;
use bytes::Bytes;
use quanta_index_core::index::{IndexError, MemoryLease};

struct AdmittedBuffer {
    // Field order releases the Arrow backing before the shared memory charge.
    buffer: Buffer,
    _lease: MemoryLease,
}

impl AsRef<[u8]> for AdmittedBuffer {
    fn as_ref(&self) -> &[u8] {
        self.buffer.as_slice()
    }
}

fn retain_buffer(buffer: Buffer, memory: &MemoryLease) -> Result<Buffer, IndexError> {
    let original = buffer.as_ptr();
    // Bytes owns the existing Arrow Buffer and one clone of the same charge.
    // No payload bytes move or copy; a sliced buffer keeps its original view.
    let retained = Buffer::from(Bytes::from_owner(AdmittedBuffer {
        buffer,
        _lease: memory.clone(),
    }));
    if retained.as_ptr() != original {
        return Err(IndexError::BackendViolation(
            "ANN admitted Arrow buffer changed its backing pointer".into(),
        ));
    }
    Ok(retained)
}

fn retain_nulls(nulls: NullBuffer, memory: &MemoryLease) -> Result<NullBuffer, IndexError> {
    let bits = nulls.into_inner();
    let offset = bits.offset();
    let len = bits.len();
    let buffer = retain_buffer(bits.into_inner(), memory)?;
    Ok(NullBuffer::new(BooleanBuffer::new(buffer, offset, len)))
}

fn retain_data(data: ArrayData, memory: &MemoryLease) -> Result<ArrayData, IndexError> {
    let (data_type, len, nulls, offset, buffers, children) = data.into_parts();
    let mut retained_buffers = Vec::new();
    retained_buffers
        .try_reserve_exact(buffers.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for buffer in buffers {
        retained_buffers.push(retain_buffer(buffer, memory)?);
    }
    let mut retained_children = Vec::new();
    retained_children
        .try_reserve_exact(children.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for child in children {
        retained_children.push(retain_data(child, memory)?);
    }
    let retained_nulls = nulls.map(|nulls| retain_nulls(nulls, memory)).transpose()?;
    ArrayData::builder(data_type)
        .len(len)
        .offset(offset)
        .nulls(retained_nulls)
        .buffers(retained_buffers)
        .child_data(retained_children)
        .build()
        .map_err(|error| {
            IndexError::BackendViolation(format!("ANN admitted Arrow data changed shape: {error}"))
        })
}

/// Tie all physical data, child, and validity buffers to the already admitted
/// charge. This only shares the original lease; it issues no second grant.
pub(super) fn retain_batch(
    batch: RecordBatch,
    memory: Option<MemoryLease>,
) -> Result<RecordBatch, IndexError> {
    let Some(memory) = memory else {
        return Ok(batch);
    };
    let (schema, source_columns, row_count) = batch.into_parts();
    let mut columns = Vec::new();
    columns
        .try_reserve_exact(source_columns.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for column in source_columns {
        columns.push(make_array(retain_data(column.to_data(), &memory)?));
    }
    let options = RecordBatchOptions::new().with_row_count(Some(row_count));
    RecordBatch::try_new_with_options(schema, columns, &options).map_err(|error| {
        IndexError::BackendViolation(format!("ANN admitted Arrow batch changed shape: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use arrow_array::{Array as _, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch};
    use arrow_buffer::Buffer;
    use arrow_schema::{DataType, Field, Schema};
    use quanta_index_core::index::{MemoryLease, MemoryLeaseOwner};

    use super::retain_batch;

    struct Releases(Arc<AtomicUsize>);

    impl MemoryLeaseOwner for Releases {
        fn release(&self, _bytes: u64) {
            let _previous = self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn nested_batch() -> Result<RecordBatch, Box<dyn std::error::Error>> {
        let values: ArrayRef = Arc::new(Float32Array::from(vec![1.0f32, 2.0]));
        let list = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            2,
            values,
            None,
        )?;
        let schema = Arc::new(Schema::new(vec![Field::new(
            "vector",
            list.data_type().clone(),
            false,
        )]));
        Ok(RecordBatch::try_new(schema, vec![Arc::new(list)])?)
    }

    fn charge(released: &Arc<AtomicUsize>) -> MemoryLease {
        MemoryLease::new(64, Arc::new(Releases(Arc::clone(released))))
    }

    fn child_buffer(batch: &RecordBatch) -> Result<Buffer, Box<dyn std::error::Error>> {
        let column = batch.columns().first().ok_or("fixture column absent")?;
        let data = column.to_data();
        let child = data.child_data().first().ok_or("fixture child absent")?;
        Ok(child
            .buffers()
            .first()
            .ok_or("fixture child buffer absent")?
            .clone())
    }

    #[test]
    fn last_child_buffer_keeps_original_charge_without_copy()
    -> Result<(), Box<dyn std::error::Error>> {
        let released = Arc::new(AtomicUsize::new(0));
        let batch = nested_batch()?;
        let original_ptr = child_buffer(&batch)?.as_ptr();

        let retained = retain_batch(batch, Some(charge(&released)))?;
        let child_buffer = child_buffer(&retained)?;
        if child_buffer.as_ptr() != original_ptr {
            return Err("child Arrow buffer was copied".into());
        }
        drop(retained);
        if released.load(Ordering::SeqCst) != 0 {
            return Err("charge released before child Arrow buffer".into());
        }
        drop(child_buffer);
        if released.load(Ordering::SeqCst) != 1 {
            return Err("charge was not released by last child Arrow buffer".into());
        }
        Ok(())
    }

    #[test]
    fn detached_child_keeps_charge_after_outer_future_is_aborted()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, detached_child_body())
    }

    async fn detached_child_body() -> Result<(), Box<dyn std::error::Error>> {
        let released = Arc::new(AtomicUsize::new(0));
        let batch = nested_batch()?;
        let memory = charge(&released);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let outer = tokio::spawn(async move {
            let retained = retain_batch(batch, Some(memory))?;
            let child = tokio::spawn(async move {
                let _sent = ready_tx.send(());
                let _released = release_rx.await;
                drop(retained);
                let _sent = done_tx.send(());
            });
            drop(child);
            std::future::pending::<Result<(), quanta_index_core::index::IndexError>>().await
        });
        ready_rx.await?;
        outer.abort();
        match outer.await {
            Err(error) if error.is_cancelled() => {}
            _ => return Err("outer future did not cancel".into()),
        }
        if released.load(Ordering::SeqCst) != 0 {
            return Err("outer cancellation released detached child's charge".into());
        }
        if release_tx.send(()).is_err() {
            return Err("detached child exited before release signal".into());
        }
        done_rx.await?;
        if released.load(Ordering::SeqCst) != 1 {
            return Err("detached child's final drop did not release charge".into());
        }
        Ok(())
    }
}
