use super::{PreparedMapOutput, SearchablePreparation, check_owner_prefix};
use crate::index::{
    AttemptRef, Digest, IndexError, MapEdit, MapRoot, ObjectBatchInput, ObjectRef, StagedMapRoots,
    StagedObjectReferences, WorkBudget,
    ports::{ObjectWriteSink, PreparedObjectMapBatch},
};

struct BackendToken {
    objects: Vec<ObjectRef>,
    roots: Vec<MapRoot>,
    prepared_root: Option<MapRoot>,
}

impl PreparedObjectMapBatch for BackendToken {
    fn root(&self) -> MapRoot {
        self.prepared_root.unwrap_or(MapRoot(None))
    }

    fn initial_objects(&self) -> &[ObjectRef] {
        &self.objects
    }

    fn initial_roots(&self) -> &[MapRoot] {
        &self.roots
    }

    fn has_prepared_root(&self) -> bool {
        self.prepared_root.is_some()
    }

    fn install(
        self: Box<Self>,
        _objects: &[ObjectBatchInput],
        _additional: Option<(MapRoot, &MapEdit)>,
        _attempt: AttemptRef,
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
        Err(IndexError::BackendViolation(
            "fake prepared backend must not install".into(),
        ))
    }
}

fn reference(byte: u8) -> ObjectRef {
    ObjectRef(Digest([byte; 32]))
}

fn root(byte: u8) -> MapRoot {
    MapRoot(Some(reference(byte)))
}

fn token(objects: &[ObjectRef], roots: &[MapRoot], prepared_root: Option<MapRoot>) -> BackendToken {
    BackendToken {
        objects: objects.to_vec(),
        roots: roots.to_vec(),
        prepared_root,
    }
}

fn searchable(token: Option<BackendToken>) -> SearchablePreparation {
    SearchablePreparation {
        reference: None,
        prepared: None,
        attempt: None,
        prepared_map: token.map(|token| -> Box<dyn PreparedObjectMapBatch> { Box::new(token) }),
    }
}

#[test]
fn owner_scope_rejects_forged_or_partial_transferred_prefix() {
    let objects = [reference(1), reference(2)];
    let roots = [root(3), root(4)];
    let matching = searchable(Some(token(&objects, &roots, Some(root(5)))));
    assert!(
        matching
            .validate_owner_maps(&objects, &roots, false)
            .is_ok()
    );
    assert!(matches!(
        matching.validate_owner_maps(&[objects[1], objects[0]], &roots, false),
        Err(IndexError::BackendViolation(_))
    ));
    assert!(matches!(
        matching.validate_owner_maps(&objects, &[roots[1], roots[0]], false),
        Err(IndexError::BackendViolation(_))
    ));
    assert!(matches!(
        matching.validate_owner_maps(&objects[..1], &roots, false),
        Err(IndexError::BackendViolation(_))
    ));

    // A backend cannot turn one missing owner slot into a valid prefix.
    let partial = searchable(Some(token(&objects[..1], &roots, Some(root(5)))));
    assert!(matches!(
        partial.validate_owner_maps(&objects, &roots, false),
        Err(IndexError::BackendViolation(_))
    ));
    let omitted_root = searchable(Some(token(&objects, &roots[..1], Some(root(5)))));
    assert!(matches!(
        omitted_root.validate_owner_maps(&objects, &roots, false),
        Err(IndexError::BackendViolation(_))
    ));

    // A pending owner cannot lose its entire prepared prefix, with or without
    // a standalone delta token.
    assert!(matches!(
        searchable(None).validate_owner_maps(&objects, &roots, false),
        Err(IndexError::BackendViolation(_))
    ));
    assert!(matches!(
        searchable(Some(token(&[], &[], Some(root(5)))))
            .validate_owner_maps(&objects, &roots, false),
        Err(IndexError::BackendViolation(_))
    ));
    // Only an already installed owner may carry an empty prefix.
    assert!(
        searchable(None)
            .validate_owner_maps(&objects, &roots, true)
            .is_ok()
    );
    assert!(
        searchable(Some(token(&[], &[], Some(root(5)))))
            .validate_owner_maps(&objects, &roots, true)
            .is_ok()
    );
    assert!(matches!(
        matching.validate_owner_maps(&objects, &roots, true),
        Err(IndexError::BackendViolation(_))
    ));
}

#[test]
fn owner_metadata_only_token_requires_two_original_ordinals() {
    let objects = [reference(1), reference(2)];
    let roots = [root(3), root(4)];
    let metadata = token(&objects, &roots, None);
    assert!(check_owner_prefix(&metadata).is_ok());
    assert!(
        searchable(Some(metadata))
            .validate_owner_maps(&objects, &roots, false)
            .is_ok()
    );
    assert!(matches!(
        check_owner_prefix(&token(&[], &[], None)),
        Err(IndexError::BackendViolation(_))
    ));
    assert!(matches!(
        check_owner_prefix(&token(&objects[..1], &roots, None)),
        Err(IndexError::BackendViolation(_))
    ));

    let parent = reference(6);
    let output = PreparedMapOutput::new(&token(&objects, &roots, None), &[parent]);
    assert!(output.is_ok());
    let Ok(output) = output else {
        return;
    };
    assert!(
        output
            .check(
                &[objects[0], objects[1], parent],
                &[roots[0], roots[1], root(7)],
                true
            )
            .is_ok()
    );
    assert!(matches!(
        output.check(&[objects[0], parent], &[roots[0], roots[1], root(7)], true),
        Err(IndexError::BackendViolation(_))
    ));
}

#[test]
fn prepared_output_rejects_reordered_or_truncated_backend_ordinals() {
    let metadata = [reference(1), reference(2)];
    let appended = [reference(6), reference(7)];
    let owners = [root(3), root(4)];
    let delta = root(5);
    let ordinary = PreparedMapOutput::new(&token(&[], &[], Some(delta)), &appended);
    assert!(ordinary.is_ok());
    let Ok(ordinary) = ordinary else {
        return;
    };
    assert!(ordinary.check(&appended, &[delta, root(8)], true).is_ok());
    let output = PreparedMapOutput::new(&token(&metadata, &owners, Some(delta)), &appended);
    assert!(output.is_ok());
    let Ok(output) = output else {
        return;
    };
    let objects = [metadata[0], metadata[1], appended[0], appended[1]];
    let roots = [owners[0], owners[1], delta, root(8)];
    assert!(output.check(&objects, &roots, true).is_ok());
    for wrong in [
        vec![metadata[1], metadata[0], appended[0], appended[1]],
        vec![metadata[0], metadata[1], appended[1], appended[0]],
        vec![metadata[0], metadata[1], appended[0]],
        vec![
            metadata[0],
            metadata[1],
            appended[0],
            appended[1],
            reference(9),
        ],
    ] {
        assert!(matches!(
            output.check(&wrong, &roots, true),
            Err(IndexError::BackendViolation(_))
        ));
    }
    for wrong in [
        vec![owners[1], owners[0], delta, root(8)],
        vec![owners[0], owners[1], root(9), root(8)],
        vec![owners[0], owners[1], delta],
        vec![owners[0], owners[1], delta, root(8), root(9)],
    ] {
        assert!(matches!(
            output.check(&objects, &wrong, true),
            Err(IndexError::BackendViolation(_))
        ));
    }
    assert!(matches!(
        PreparedMapOutput::new(
            &token(&metadata, &owners, Some(delta)),
            &[reference(6), reference(7), reference(8)],
        ),
        Err(IndexError::ResourceLimited)
    ));
    assert!(matches!(
        PreparedMapOutput::new(&token(&metadata, &owners, Some(MapRoot(None))), &appended),
        Err(IndexError::BackendViolation(_))
    ));
}
