//! Optional adoption of two physical packets under the existing union witness.

use super::*;
use crate::native::text_spool::ConfirmedOccurrencePrograms;
use quanta_index_core::index::MemoryLifetime;

impl OccurrenceWrites<'_> {
    pub(super) fn flush_before_input(
        &mut self,
        reference: ObjectRef,
        bytes: usize,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
        confirmed: &mut ConfirmedOccurrences,
        edits: &mut InventoryEdits,
    ) -> Result<(), IndexError> {
        if self.needs_flush(reference, bytes)? && !self.try_start_run(bytes, context, confirmed)? {
            self.flush(context, spool, confirmed, edits)?;
        }
        Ok(())
    }

    fn try_start_run(
        &mut self,
        next_bytes: usize,
        context: &mut AttemptContext<'_>,
        confirmed: &mut ConfirmedOccurrences,
    ) -> Result<bool, IndexError> {
        context.budget.checkpoint()?;
        if self.prepared_run.is_some() {
            return Ok(false);
        }
        // Adoption must not normalize an invalid original packet that the
        // ordinary flush would reject before installation.
        if self.inputs.len() != self.pending.len() || self.inputs.len() != self.payload_memory.len()
        {
            return Err(IndexError::BackendViolation(
                "native occurrence batch custody differs".into(),
            ));
        }
        if self.confirmation_programs.is_some() {
            return Err(IndexError::BackendViolation(
                "original occurrence packet has union confirmation backing".into(),
            ));
        }
        let maximum_payload = crate::native::text_occurrence::block::MAX_BLOCK_BYTES;
        let Some(objects) = self.install_objects else {
            return Ok(false);
        };
        if self.pending.is_empty()
            || self.pending.len() >= quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH
            || next_bytes > maximum_payload
            || self
                .inputs
                .iter()
                .any(|input| !input.children.is_empty() || input.payload.len() > maximum_payload)
        {
            return Ok(false);
        }
        // This is mandatory even on the original immediately flushed path.
        confirmed.prepare(context.attempt, context.budget)?;
        let programs = if confirmed.limit == 0 {
            None
        } else {
            let Some(programs) = ConfirmedOccurrencePrograms::try_new(context.budget)? else {
                return Ok(false);
            };
            Some(programs)
        };
        let capacity = quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH;
        let Some(memory) = context.budget.try_reserve_memory_for_optimization(
            Self::metadata_bytes(capacity)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(false);
        };
        let mut inputs = Vec::new();
        let mut pending = Vec::new();
        let mut payload_memory = Vec::new();
        if inputs.try_reserve_exact(capacity).is_err()
            || pending.try_reserve_exact(capacity).is_err()
            || payload_memory.try_reserve_exact(capacity).is_err()
            || inputs.capacity() > capacity
            || pending.capacity() > capacity
            || payload_memory.capacity() > capacity
        {
            return Ok(false);
        }
        let Some(mut prepared) = objects.try_prepare_leaf_install_run(
            context.attempt,
            maximum_payload,
            usize::try_from(MAX_OCCURRENCE_STAGED_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?,
            context.budget,
        )?
        else {
            return Ok(false);
        };
        // Optional decline is complete before any adoption, work charge, or
        // persistent effect. There is no fallback after this DATA is adopted.
        for (input, expected) in self.inputs.iter().zip(&self.pending) {
            if !prepared.accepts(input.payload.len()) {
                return Err(IndexError::BackendViolation(
                    "prepared run rejected its admitted prefix".into(),
                ));
            }
            prepared.push(&input.payload, expected.identified, context.budget)?;
        }
        if !prepared.accepts(next_bytes) {
            return Err(IndexError::BackendViolation(
                "prepared run cannot hold the admitted successor".into(),
            ));
        }
        pending.append(&mut self.pending);
        // Original payloads die only after the exact canonical wire has taken
        // custody. No released grant is reacquired for an independent copy.
        self.inputs.clear();
        self.payload_memory.clear();
        self.inputs = inputs;
        self.pending = pending;
        self.payload_memory = payload_memory;
        self.metadata_memory = Some(memory);
        self.confirmation_programs = programs;
        self.prepared_run = Some(prepared);
        Ok(true)
    }
}
