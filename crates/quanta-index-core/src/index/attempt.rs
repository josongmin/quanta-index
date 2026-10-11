use super::{
    AttemptContext, IndexError, MapBatchRequest, MapEdit, MapRoot, ObjectBatchInput, ObjectRef,
    RegisteredObject, ports,
};
use quanta_index_contract::index::AttemptRef;

impl AttemptContext<'_> {
    pub fn put(&mut self, payload: &[u8], children: &[ObjectRef]) -> Result<ObjectRef, IndexError> {
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        let object = self
            .objects
            .install(payload, children, &sink, self.budget)?;
        Ok(object.reference)
    }

    pub fn apply(&mut self, base: MapRoot, edits: &[MapEdit]) -> Result<MapRoot, IndexError> {
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        let result = self.maps.apply(base, edits, &sink, self.budget)?;
        Ok(result.root)
    }

    pub fn apply_batch(
        &mut self,
        requests: &[MapBatchRequest],
    ) -> Result<Vec<MapRoot>, IndexError> {
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        let result = self.maps.apply_batch(requests, &sink, self.budget)?;
        if result.roots.len() != requests.len() {
            return Err(IndexError::BackendViolation(
                "map batch lost ordinal output".into(),
            ));
        }
        Ok(result.roots)
    }

    pub fn apply_stream(
        &mut self,
        bases: &[MapRoot],
        edits: &mut dyn ports::MapEditCursor,
    ) -> Result<Vec<MapRoot>, IndexError> {
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        let roots = self.maps.apply_stream(bases, edits, &sink, self.budget)?;
        if roots.len() != bases.len() {
            return Err(IndexError::BackendViolation(
                "streamed map lost ordinal output".into(),
            ));
        }
        Ok(roots)
    }

    pub fn put_batch(&mut self, inputs: &[ObjectBatchInput]) -> Result<Vec<ObjectRef>, IndexError> {
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        let objects = self.objects.install_batch(inputs, &sink, self.budget)?;
        if objects.len() != inputs.len() {
            return Err(IndexError::BackendViolation(
                "object batch lost ordinal output".into(),
            ));
        }
        Ok(objects.into_iter().map(|object| object.reference).collect())
    }
}

struct AttemptSink<'a> {
    attempt: AttemptRef,
    custody: &'a dyn ports::ObjectCustodyPort,
}

impl ports::ObjectWriteSink for AttemptSink<'_> {
    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.custody
            .reserve_objects(self.attempt, std::slice::from_ref(object))
    }
    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.custody
            .register_objects(self.attempt, std::slice::from_ref(object))
    }
    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.custody.reserve_objects(self.attempt, objects)
    }
    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.custody.register_objects(self.attempt, objects)
    }
}
