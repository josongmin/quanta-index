use std::cell::{Cell, RefCell};

use quanta_index_contract_base::{
    DecodeAttemptStateV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1, DecodeFieldKindV1,
    DecodeFieldTokenV1, ParserTextInputErrorV1, SourceIdentityDecodeAdmissionV1,
};
use serde::{
    Deserializer,
    de::{self, DeserializeSeed},
};

use super::{DirtyMutation, StructuralReplaceScope};
use crate::{RepoId, RevisionId, ipc::ingest::CorpusDecodeLimitsV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Semantic fixture only: identity and parser ownership remain the production
// adapter's responsibility. The fixed nested wires contain no repo identity.
struct TestAdmission {
    attempt: RefCell<DecodeAttemptStateV1>,
    aborted: Cell<bool>,
    text_reservations: Cell<usize>,
    boxed_copies: Cell<usize>,
    sequence_slots: Cell<usize>,
    deny_text: bool,
    deny_box: bool,
}

impl TestAdmission {
    fn new(deny_text: bool, deny_box: bool) -> Self {
        Self {
            attempt: RefCell::new(DecodeAttemptStateV1::default()),
            aborted: Cell::new(false),
            text_reservations: Cell::new(0),
            boxed_copies: Cell::new(0),
            sequence_slots: Cell::new(0),
            deny_text,
            deny_box,
        }
    }
}

impl DecodeBackingAdmissionV1 for TestAdmission {
    fn abort_decode(&self) {
        self.aborted.set(true);
        self.attempt.borrow_mut().abort_v1();
    }

    fn enter_decode_field_v1(
        &self,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        let mut attempt = self.attempt.borrow_mut();
        if !attempt.is_reading_v1() {
            attempt.begin_decode_v1()?;
        }
        attempt.enter_field_v1(kind, maximum)
    }

    fn leave_decode_field_v1(
        &self,
        token: DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.attempt.borrow_mut().leave_field_v1(token)
    }

    fn reserve_text(
        &self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.get() || self.deny_text {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)?;
        self.text_reservations.set(
            self.text_reservations
                .get()
                .checked_add(1)
                .ok_or(DecodeBackingRefusalV1)?,
        );
        Ok(())
    }

    fn reserve_bytes(
        &self,
        _destination: &mut Vec<u8>,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.get() {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)?;
        self.sequence_slots.set(
            self.sequence_slots
                .get()
                .checked_add(additional)
                .ok_or(DecodeBackingRefusalV1)?,
        );
        Ok(())
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        if self.aborted.get() || self.deny_box {
            return Err(DecodeBackingRefusalV1);
        }
        let output = source.into();
        self.boxed_copies.set(
            self.boxed_copies
                .get()
                .checked_add(1)
                .ok_or(DecodeBackingRefusalV1)?,
        );
        Ok(output)
    }

    fn adopt_parser_text<E>(
        &self,
        _source: &mut Option<String>,
        _validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, ParserTextInputErrorV1<E>> {
        Err(DecodeBackingRefusalV1.into())
    }

    fn copy_boxed_parser_text<E>(
        &self,
        _source: &mut Option<String>,
        _validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, ParserTextInputErrorV1<E>> {
        Err(DecodeBackingRefusalV1.into())
    }

    fn verify_parser_owned_bytes(
        &self,
        _source: &[u8],
        _capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }
}

impl SourceIdentityDecodeAdmissionV1 for TestAdmission {
    fn decode_repo_id_v1<'de, D: Deserializer<'de>>(
        &self,
        _decoder: D,
    ) -> Result<RepoId, D::Error> {
        Err(de::Error::custom(
            "identity requires production Original owner",
        ))
    }

    fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
        &self,
        _decoder: D,
    ) -> Result<RevisionId, D::Error> {
        Err(de::Error::custom(
            "identity requires production Original owner",
        ))
    }
}

fn limits() -> CorpusDecodeLimitsV1 {
    CorpusDecodeLimitsV1 {
        text_bytes: 64,
        source_bytes: 64,
        bundle_bytes: 64,
        collection_items: 8,
    }
}

const DIRTY_UPSERT: &str = concat!(
    r#"{"Upsert":{"wire_version":1,"doc_id":"d1","applied_at_ms":7,"payload_hash":["#,
    "0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}}",
);

const STRUCTURAL_SCOPE: &str = concat!(
    r#"{"scope":{"doc_surface":"File","repo_relative_path":"src/a.rs"},"scope_digest":"abc","trees":[{"chunk_id":"d1","record":{"wire_version":1,"lang":"rust","root":{"kind":"function_item","byte_start":0,"byte_end":2,"children":[{"kind":"identifier","byte_start":0,"byte_end":1,"children":[]}]},"source_hash":["#,
    r#"0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"role_tag_schema_version":1,"role_tags":[{"role":"definition","byte_start":0,"byte_end":2}]}}]}"#,
);

#[test]
fn dirty_upsert_admitted_matches_fixed_ordinary_wire() -> TestResult {
    let ordinary: DirtyMutation = serde_json::from_str(DIRTY_UPSERT)?;
    let admission = TestAdmission::new(false, false);
    let mut decoder = serde_json::Deserializer::from_str(DIRTY_UPSERT);
    let admitted = DirtyMutation::deserialize_seed_with_admission_v1(&admission, limits())
        .deserialize(&mut decoder)?;
    if admitted != ordinary || admission.text_reservations.get() != 1 || admission.aborted.get() {
        return Err("dirty upsert admitted path changed the fixed wire".into());
    }
    Ok(())
}

#[test]
fn structural_nested_tree_admitted_matches_fixed_ordinary_wire() -> TestResult {
    let ordinary: StructuralReplaceScope = serde_json::from_str(STRUCTURAL_SCOPE)?;
    let admission = TestAdmission::new(false, false);
    let mut decoder = serde_json::Deserializer::from_str(STRUCTURAL_SCOPE);
    let admitted = StructuralReplaceScope::deserialize_seed_with_admission_v1(&admission, limits())
        .deserialize(&mut decoder)?;
    if admitted != ordinary
        || admission.boxed_copies.get() < 4
        || admission.sequence_slots.get() < 3
        || admission.aborted.get()
    {
        return Err("structural nested admission changed the fixed wire".into());
    }
    Ok(())
}

#[test]
fn denied_nested_text_refuses_before_child_or_parent_backing_birth() {
    let admission = TestAdmission::new(true, false);
    let mut decoder = serde_json::Deserializer::from_str(DIRTY_UPSERT);
    assert!(
        DirtyMutation::deserialize_seed_with_admission_v1(&admission, limits())
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(admission.aborted.get());
    assert_eq!(admission.text_reservations.get(), 0);
    assert_eq!(admission.sequence_slots.get(), 0);
}

#[test]
fn denied_parse_language_box_refuses_before_tree_vector_birth() {
    let admission = TestAdmission::new(false, true);
    let mut decoder = serde_json::Deserializer::from_str(STRUCTURAL_SCOPE);
    assert!(
        StructuralReplaceScope::deserialize_seed_with_admission_v1(&admission, limits())
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(admission.aborted.get());
    assert_eq!(admission.boxed_copies.get(), 0);
    assert_eq!(admission.sequence_slots.get(), 0);
}

#[test]
fn admitted_dirty_and_structural_reject_malformed_fixed_wires() {
    const DIRTY_DUPLICATE: &str = r#"{"Upsert":{"wire_version":1,"wire_version":2,"doc_id":"d1","applied_at_ms":7,"payload_hash":[0]}}"#;
    const STRUCTURAL_UNKNOWN: &str = r#"{"scope":{"doc_surface":"File","repo_relative_path":"a.rs"},"scope_digest":"abc","trees":[],"alien":1}"#;
    const STRUCTURAL_NULL: &str = r#"{"scope":null,"scope_digest":"abc","trees":[]}"#;

    let dirty = TestAdmission::new(false, false);
    let mut dirty_decoder = serde_json::Deserializer::from_str(DIRTY_DUPLICATE);
    assert!(serde_json::from_str::<DirtyMutation>(DIRTY_DUPLICATE).is_err());
    assert!(
        DirtyMutation::deserialize_seed_with_admission_v1(&dirty, limits())
            .deserialize(&mut dirty_decoder)
            .is_err()
    );
    assert!(dirty.aborted.get());

    for wire in [STRUCTURAL_UNKNOWN, STRUCTURAL_NULL] {
        let owner = TestAdmission::new(false, false);
        let mut decoder = serde_json::Deserializer::from_str(wire);
        assert!(serde_json::from_str::<StructuralReplaceScope>(wire).is_err());
        assert!(
            StructuralReplaceScope::deserialize_seed_with_admission_v1(&owner, limits())
                .deserialize(&mut decoder)
                .is_err()
        );
        assert!(owner.aborted.get());
    }
}
