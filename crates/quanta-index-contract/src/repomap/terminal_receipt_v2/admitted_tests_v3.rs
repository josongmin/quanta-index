//! Fixed schema oracle for the common ordinary/admitted terminal visitor.

use std::cell::{Cell, RefCell};

use quanta_index_contract_base::{
    DecodeAttemptStateV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1, DecodeFieldKindV1,
    DecodeFieldTokenV1, ParserTextInputErrorV1, SourceIdentityDecodeAdmissionV1,
};
use serde::{Deserialize, Deserializer, de::DeserializeSeed};

use super::*;

const RECEIPT_JSON: &str = r#"{
  "phase":"publish",
  "mutation":{
    "repo_id":"repo-a","revision_id":"rev-b","manifest_generation":7,
    "prior_candidate_commitment":null,"new_candidate_commitment":"candidate-c",
    "activation_epoch":0,"terminal_sequence":12,"replayed":false
  },
  "manifest_digest":"manifest-d","snapshot_id":"snapshot-e",
  "projection_version":3,"authority_digest":"authority-f",
  "source_bundle_digest":"bundle-g"
}"#;

struct TestAdmission {
    allocations: Cell<usize>,
    refuse_after: usize,
    aborted: Cell<bool>,
    attempt: RefCell<DecodeAttemptStateV1>,
}

impl TestAdmission {
    fn new(refuse_after: usize) -> Self {
        Self {
            allocations: Cell::new(0),
            refuse_after,
            aborted: Cell::new(false),
            attempt: RefCell::new(DecodeAttemptStateV1::default()),
        }
    }

    fn permit(&self) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.get() || self.allocations.get() >= self.refuse_after {
            return Err(DecodeBackingRefusalV1);
        }
        self.allocations.set(
            self.allocations
                .get()
                .checked_add(1)
                .ok_or(DecodeBackingRefusalV1)?,
        );
        Ok(())
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
        self.permit()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)
    }

    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.permit()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)
    }

    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.permit()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        self.permit()?;
        Ok(source.into())
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
    fn decode_repo_id_v1<'de, D: Deserializer<'de>>(&self, decoder: D) -> Result<RepoId, D::Error> {
        RepoId::deserialize(decoder)
    }

    fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
        &self,
        decoder: D,
    ) -> Result<RevisionId, D::Error> {
        RevisionId::deserialize(decoder)
    }
}

#[test]
fn admitted_terminal_receipt_matches_fixed_ordinary_schema() {
    let ordinary: RepoMapTerminalReceiptV2 =
        serde_json::from_str(RECEIPT_JSON).expect("fixed ordinary receipt");
    let admission = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(RECEIPT_JSON);
    let admitted = RepoMapTerminalReceiptV2::deserialize_seed_with_admission_v3(&admission, 128)
        .deserialize(&mut decoder)
        .expect("admitted fixed receipt");
    assert_eq!(admitted, ordinary);
    assert_eq!(admitted.mutation.terminal_sequence, 12);
    assert_eq!(admission.allocations.get(), 5);
    assert!(!admission.aborted.get());
}

#[test]
fn admitted_terminal_receipt_refuses_text_birth_before_result() {
    let admission = TestAdmission::new(0);
    let mut decoder = serde_json::Deserializer::from_str(RECEIPT_JSON);
    let refused = RepoMapTerminalReceiptV2::deserialize_seed_with_admission_v3(&admission, 128)
        .deserialize(&mut decoder);
    assert!(refused.is_err());
    assert!(admission.aborted.get());
}
