use super::{
    AttemptContext, IndexError, MapBatchRequest, MapEdit, MapRoot, MemoryLifetime,
    ObjectBatchInput, ObjectInstallScope, ObjectRef, RegisteredObject, SearchableParentPreparation,
    SearchablePreparation, StagedObjectBatch, StagedObjectReferences, StagedPayload, ports,
};
use quanta_index_contract::index::AttemptRef;

impl AttemptContext<'_> {
    fn install_prepared_map(
        &mut self,
        prepared: Box<dyn ports::PreparedObjectMapBatch>,
        inputs: &[ObjectBatchInput],
        additional: Option<(MapRoot, &MapEdit)>,
    ) -> Result<(StagedObjectReferences, crate::index::StagedMapRoots), IndexError> {
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        prepared.install(inputs, additional, self.attempt, &sink, self.budget)
    }

    pub fn put_and_apply(
        &mut self,
        writer: &dyn ports::ObjectMapBatchPort,
        inputs: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
    ) -> Result<(StagedObjectReferences, crate::index::StagedMapRoots), IndexError> {
        if inputs.is_empty()
            || inputs.len() > ports::MAX_OBJECT_MAP_BATCH_OBJECTS
            || bases.is_empty()
            || bases.len() > ports::MAX_OBJECT_MAP_BATCH_ROOTS
            || edits.len() != bases.len()
        {
            return Err(IndexError::ResourceLimited);
        }
        let sink = AttemptSink {
            attempt: self.attempt,
            custody: self.custody,
        };
        let result = writer.install_objects_and_maps(inputs, bases, edits, &sink, self.budget)?;
        if result.0.len() != inputs.len() || result.1.len() != bases.len() {
            return Err(IndexError::BackendViolation(
                "object/map batch lost ordinal output".into(),
            ));
        }
        Ok(result)
    }

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
    ) -> Result<crate::index::StagedMapRoots, IndexError> {
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
    ) -> Result<crate::index::StagedMapRoots, IndexError> {
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

    pub fn put_batch(
        &mut self,
        inputs: &[ObjectBatchInput],
    ) -> Result<StagedObjectReferences, IndexError> {
        let bytes = inputs
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            self.budget.checkpoint()?;
            None
        } else {
            self.budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
        };
        let mut references = Vec::new();
        references
            .try_reserve_exact(inputs.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        if inputs.is_empty() {
            return StagedObjectReferences::from_parts(references, memory);
        }
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
        references.extend(objects.into_iter().map(|object| object.reference));
        StagedObjectReferences::from_parts(references, memory)
    }
}

struct AttemptSink<'a> {
    attempt: AttemptRef,
    custody: &'a dyn ports::ObjectCustodyPort,
}

impl ports::ObjectWriteSink for AttemptSink<'_> {
    fn managed_scope(&self) -> Option<ObjectInstallScope> {
        Some(ObjectInstallScope::Attempt(self.attempt))
    }

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

// The opaque preparation type stays independent of execution ports.
impl SearchablePreparation {
    pub fn object(
        payload: StagedPayload,
        children: &[ObjectRef],
        context: &mut AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        let reference = context
            .objects
            .identify(&payload.bytes, children, context.budget)?
            .reference;
        // The child and its coverage parent use the original two-object batch.
        let mut inputs = StagedObjectBatch::new(2, context.budget)?;
        inputs.push(payload, children, context.budget)?;
        Ok(Self {
            reference: Some(reference),
            prepared: Some(inputs),
            attempt: Some(context.attempt),
            prepared_map: None,
        })
    }

    /// Bind one original prepared map to an admitted searchable descriptor.
    /// Its provisional root must already be an explicit descriptor child.
    pub fn with_prepared_map(
        mut self,
        prepared_map: Box<dyn ports::PreparedObjectMapBatch>,
        context: &AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        self.check_attempt(context)?;
        if self.prepared_map.is_some() {
            return Err(IndexError::BackendViolation(
                "searchable map was already prepared".into(),
            ));
        }
        check_owner_prefix(prepared_map.as_ref())?;
        if !prepared_map.has_prepared_root() {
            return Err(IndexError::BackendViolation(
                "searchable delta token has no prepared root".into(),
            ));
        }
        let root = prepared_map.root().0.ok_or_else(|| {
            IndexError::BackendViolation("prepared searchable map has no root".into())
        })?;
        let descriptor = self
            .prepared
            .as_ref()
            .and_then(|inputs| inputs.inputs().first())
            .ok_or_else(|| {
                IndexError::BackendViolation("prepared map requires an original descriptor".into())
            })?;
        if !descriptor.children.contains(&root) {
            return Err(IndexError::BackendViolation(
                "searchable descriptor omitted its prepared map root".into(),
            ));
        }
        self.prepared_map = Some(prepared_map);
        Ok(self)
    }

    /// Retain the selected owner's unchanged metadata when no delta took it.
    pub fn with_owner_maps(
        mut self,
        prepared_map: Box<dyn ports::PreparedObjectMapBatch>,
        context: &AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        self.check_attempt(context)?;
        check_owner_prefix(prepared_map.as_ref())?;
        if self.prepared_map.is_some()
            || prepared_map.has_prepared_root()
            || prepared_map.initial_objects().len() != 2
        {
            return Err(IndexError::BackendViolation(
                "searchable owner token has invalid shape".into(),
            ));
        }
        let owner = prepared_map
            .initial_roots()
            .first()
            .and_then(|root| root.0)
            .ok_or_else(|| IndexError::BackendViolation("prepared owner map has no root".into()))?;
        let bound = match self.prepared.as_ref() {
            Some(inputs) => inputs
                .inputs()
                .first()
                .is_some_and(|descriptor| descriptor.children.contains(&owner)),
            None => self.reference == Some(owner),
        };
        if !bound {
            return Err(IndexError::BackendViolation(
                "searchable omitted its prepared owner root".into(),
            ));
        }
        self.attempt = Some(context.attempt);
        self.prepared_map = Some(prepared_map);
        Ok(self)
    }

    /// Compare any transferred owner prefix with the scope checked by Kernel.
    /// An empty prefix belongs to the original path after an early flush.
    pub fn validate_owner_maps(
        &self,
        objects: &[ObjectRef],
        roots: &[MapRoot],
        installed: bool,
    ) -> Result<(), IndexError> {
        if objects.len() != 2 || roots.len() != 2 {
            return Err(IndexError::BackendViolation(
                "selected owner scope has invalid shape".into(),
            ));
        }
        if let Some(token) = self.prepared_map.as_ref() {
            check_owner_prefix(token.as_ref())?;
            let valid = if installed {
                token.initial_objects().is_empty() && token.initial_roots().is_empty()
            } else {
                token.initial_objects() == objects && token.initial_roots() == roots
            };
            if !valid {
                return Err(IndexError::BackendViolation(
                    "searchable owner token differs from selected scope".into(),
                ));
            }
        } else if !installed {
            return Err(IndexError::BackendViolation(
                "searchable lost its prepared owner token".into(),
            ));
        }
        Ok(())
    }

    fn check_attempt(&self, context: &AttemptContext<'_>) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self
            .attempt
            .is_some_and(|attempt| attempt != context.attempt)
        {
            return Err(IndexError::BackendViolation(
                "searchable preparation belongs to another attempt".into(),
            ));
        }
        Ok(())
    }

    pub fn install(
        self,
        context: &mut AttemptContext<'_>,
    ) -> Result<Option<ObjectRef>, IndexError> {
        self.check_attempt(context)?;
        if let Some(prepared_map) = self.prepared_map {
            let references = self.reference.as_slice();
            let expected = if self.prepared.is_some() {
                references
            } else {
                &[]
            };
            if let Some(inputs) = self.prepared.as_ref() {
                check_prepared_identities(inputs, expected, context)?;
            }
            let output = PreparedMapOutput::new(prepared_map.as_ref(), expected)?;
            let inputs = self
                .prepared
                .as_ref()
                .map_or(&[][..], |inputs| inputs.inputs());
            let (actual, roots) = context.install_prepared_map(prepared_map, inputs, None)?;
            output.check(&actual, &roots, false)?;
        } else if let Some(inputs) = self.prepared {
            let reference = self.reference.ok_or_else(|| {
                IndexError::BackendViolation("prepared searchable has no identity".into())
            })?;
            install_checked(&inputs, &[reference], context)?;
        }
        Ok(self.reference)
    }

    fn parent_identity(
        &self,
        payload: &StagedPayload,
        children: &[ObjectRef],
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        self.check_attempt(context)?;
        if self.prepared.is_some() {
            let child = self.reference.ok_or_else(|| {
                IndexError::BackendViolation("prepared searchable has no identity".into())
            })?;
            if !children.contains(&child) {
                return Err(IndexError::BackendViolation(
                    "searchable parent omitted its child".into(),
                ));
            }
        }
        Ok(context
            .objects
            .identify(&payload.bytes, children, context.budget)?
            .reference)
    }

    /// Retain admitted DATA so a dependent map can join the same installation.
    pub fn prepare_parent(
        self,
        payload: StagedPayload,
        children: &[ObjectRef],
        context: &mut AttemptContext<'_>,
    ) -> Result<SearchableParentPreparation, IndexError> {
        let parent = self.parent_identity(&payload, children, context)?;
        let expected_child = if self.prepared.is_some() {
            Some(self.reference.ok_or_else(|| {
                IndexError::BackendViolation("prepared searchable has no identity".into())
            })?)
        } else {
            None
        };
        let mut inputs = match self.prepared {
            Some(inputs) => inputs,
            None => StagedObjectBatch::new(1, context.budget)?,
        };
        inputs.push(payload, children, context.budget)?;
        Ok(SearchableParentPreparation {
            reference: parent,
            expected_child,
            inputs,
            attempt: self.attempt,
            prepared_map: self.prepared_map,
        })
    }

    /// Install the prepared child and canonical parent in one original group.
    pub fn install_parent(
        self,
        payload: StagedPayload,
        children: &[ObjectRef],
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        if self.prepared.is_none() && self.prepared_map.is_none() {
            let parent = self.parent_identity(&payload, children, context)?;
            let actual = context.put(&payload.bytes, children)?;
            if actual != parent {
                return Err(IndexError::BackendViolation(
                    "searchable parent identity differs".into(),
                ));
            }
            return Ok(parent);
        }
        self.prepare_parent(payload, children, context)?
            .install_without_map(context)
    }
}

impl SearchableParentPreparation {
    fn expected_references(&self) -> ([ObjectRef; 2], usize) {
        self.expected_child
            .map_or(([self.reference, self.reference], 1), |child| {
                ([child, self.reference], 2)
            })
    }

    fn check_attempt(&self, context: &AttemptContext<'_>) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self
            .attempt
            .is_some_and(|attempt| attempt != context.attempt)
        {
            return Err(IndexError::BackendViolation(
                "searchable parent belongs to another attempt".into(),
            ));
        }
        Ok(())
    }

    /// Install a prepared delta and coverage edit in one original group when
    /// present; otherwise preserve the selected writer or scalar original path.
    pub fn install_with_map(
        self,
        writer: Option<&dyn ports::ObjectMapBatchPort>,
        base: MapRoot,
        edit: &MapEdit,
        context: &mut AttemptContext<'_>,
    ) -> Result<MapRoot, IndexError> {
        self.check_attempt(context)?;
        if edit.next != Some(self.reference.into()) {
            return Err(IndexError::BackendViolation(
                "searchable coverage edit omitted its prepared parent".into(),
            ));
        }
        let (expected, count) = self.expected_references();
        let expected_refs = expected.get(..count).ok_or(IndexError::ResourceLimited)?;
        if let Some(prepared_map) = self.prepared_map {
            check_prepared_identities(&self.inputs, expected_refs, context)?;
            let output = PreparedMapOutput::new(prepared_map.as_ref(), expected_refs)?;
            let (references, roots) = context.install_prepared_map(
                prepared_map,
                self.inputs.inputs(),
                Some((base, edit)),
            )?;
            output.check(&references, &roots, true)?;
            return roots.last().copied().ok_or_else(|| {
                IndexError::BackendViolation("coverage root was not installed".into())
            });
        }
        let roots = if let Some(writer) = writer {
            let (references, roots) =
                context.put_and_apply(writer, self.inputs.inputs(), &[base], &[edit])?;
            if &*references != expected_refs || roots.len() != 1 {
                return Err(IndexError::BackendViolation(
                    "searchable parent changed ordinal output".into(),
                ));
            }
            roots
        } else {
            if count == 2 {
                install_checked(&self.inputs, expected_refs, context)?;
            } else {
                let input = self.inputs.inputs().first().ok_or_else(|| {
                    IndexError::BackendViolation("searchable parent input missing".into())
                })?;
                let actual = context.put(&input.payload, &input.children)?;
                if actual != self.reference {
                    return Err(IndexError::BackendViolation(
                        "searchable parent identity differs".into(),
                    ));
                }
            }
            let root = context.apply(base, std::slice::from_ref(edit))?;
            return Ok(root);
        };
        roots
            .first()
            .copied()
            .ok_or_else(|| IndexError::BackendViolation("coverage root was not installed".into()))
    }

    fn install_without_map(
        self,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        self.check_attempt(context)?;
        let (expected, count) = self.expected_references();
        let expected_refs = expected.get(..count).ok_or(IndexError::ResourceLimited)?;
        if let Some(prepared_map) = self.prepared_map {
            let output = PreparedMapOutput::new(prepared_map.as_ref(), expected_refs)?;
            check_prepared_identities(&self.inputs, expected_refs, context)?;
            let (references, roots) =
                context.install_prepared_map(prepared_map, self.inputs.inputs(), None)?;
            output.check(&references, &roots, false)?;
        } else if count == 2 {
            install_checked(&self.inputs, expected_refs, context)?;
        } else {
            let input = self.inputs.inputs().first().ok_or_else(|| {
                IndexError::BackendViolation("searchable parent input missing".into())
            })?;
            let actual = context.put(&input.payload, &input.children)?;
            if actual != self.reference {
                return Err(IndexError::BackendViolation(
                    "searchable parent identity differs".into(),
                ));
            }
        }
        Ok(self.reference)
    }
}

fn check_owner_prefix(token: &dyn ports::PreparedObjectMapBatch) -> Result<(), IndexError> {
    if !matches!(
        (token.initial_objects().len(), token.initial_roots().len()),
        (0, 0) | (2, 2)
    ) || (!token.has_prepared_root() && token.initial_roots().is_empty())
    {
        return Err(IndexError::BackendViolation(
            "prepared map owner prefix has invalid shape".into(),
        ));
    }
    Ok(())
}

/// Snapshot bounded ordinals before consuming the backend token. No allocation
/// or backend callback is needed after installation to authenticate its output.
struct PreparedMapOutput {
    objects: [Option<ObjectRef>; ports::MAX_OWNER_MAP_BATCH_OBJECTS],
    object_count: usize,
    roots: [MapRoot; ports::MAX_OWNER_MAP_BATCH_ROOTS],
    root_count: usize,
}

impl PreparedMapOutput {
    fn new(
        token: &dyn ports::PreparedObjectMapBatch,
        appended: &[ObjectRef],
    ) -> Result<Self, IndexError> {
        check_owner_prefix(token)?;
        let object_count = token
            .initial_objects()
            .len()
            .checked_add(appended.len())
            .ok_or(IndexError::ResourceLimited)?;
        let root_count = token
            .initial_roots()
            .len()
            .checked_add(usize::from(token.has_prepared_root()))
            .ok_or(IndexError::ResourceLimited)?;
        if object_count > ports::MAX_OWNER_MAP_BATCH_OBJECTS
            || root_count > ports::MAX_OWNER_MAP_BATCH_ROOTS
        {
            return Err(IndexError::ResourceLimited);
        }
        let mut result = Self {
            objects: [None; ports::MAX_OWNER_MAP_BATCH_OBJECTS],
            object_count,
            roots: [MapRoot(None); ports::MAX_OWNER_MAP_BATCH_ROOTS],
            root_count,
        };
        for (slot, reference) in result
            .objects
            .iter_mut()
            .zip(token.initial_objects().iter().chain(appended))
        {
            *slot = Some(*reference);
        }
        result
            .roots
            .get_mut(..token.initial_roots().len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(token.initial_roots());
        if token.has_prepared_root() {
            let root = token.root();
            if root.0.is_none() {
                return Err(IndexError::BackendViolation(
                    "prepared searchable map has no root".into(),
                ));
            }
            let index = root_count
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?;
            *result
                .roots
                .get_mut(index)
                .ok_or(IndexError::ResourceLimited)? = root;
        }
        Ok(result)
    }

    fn check(
        &self,
        objects: &[ObjectRef],
        roots: &[MapRoot],
        additional: bool,
    ) -> Result<(), IndexError> {
        if objects.len() != self.object_count
            || objects
                .iter()
                .zip(&self.objects)
                .any(|(actual, expected)| Some(*actual) != *expected)
            || Some(roots.len()) != self.root_count.checked_add(usize::from(additional))
            || roots.len() > ports::MAX_OWNER_MAP_BATCH_ROOTS
            || roots.get(..self.root_count) != self.roots.get(..self.root_count)
            || (additional && roots.last().is_none_or(|root| root.0.is_none()))
        {
            return Err(IndexError::BackendViolation(
                "prepared searchable map changed ordinal output".into(),
            ));
        }
        Ok(())
    }
}

fn check_prepared_identities(
    inputs: &StagedObjectBatch,
    expected: &[ObjectRef],
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if inputs.inputs().len() != expected.len() {
        return Err(IndexError::BackendViolation(
            "searchable prepared input shape differs".into(),
        ));
    }
    for (input, reference) in inputs.inputs().iter().zip(expected) {
        let identified =
            context
                .objects
                .identify(&input.payload, &input.children, context.budget)?;
        if identified.reference != *reference {
            return Err(IndexError::BackendViolation(
                "searchable prepared identity differs".into(),
            ));
        }
    }
    Ok(())
}

fn install_checked(
    inputs: &StagedObjectBatch,
    expected: &[ObjectRef],
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if inputs.inputs().len() != expected.len() {
        return Err(IndexError::BackendViolation(
            "searchable batch input shape differs".into(),
        ));
    }
    let actual = context.put_batch(inputs.inputs())?;
    if &*actual != expected {
        return Err(IndexError::BackendViolation(
            "searchable batch identity or order differs".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
