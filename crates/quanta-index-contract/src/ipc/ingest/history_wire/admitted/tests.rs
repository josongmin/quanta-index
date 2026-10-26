use std::cell::{Cell, RefCell};

use quanta_index_contract_base::{
    DecodeAttemptStateV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1, DecodeFieldKindV1,
    DecodeFieldTokenV1, RepoId, RevisionId, SourceIdentityDecodeAdmissionV1,
};
use serde::{Deserialize, Deserializer, de::DeserializeSeed};

use super::{
    FileContributorIngestBatch, HistoryDecodeLimitsV1, HistoryIngestBatch,
    RepoCommitRecencyIngestBatch,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// This test owner checks wire semantics and call ordering. It does not issue
// production Original/NFC DATA or certify parser provenance.
struct TestAdmission {
    attempts: RefCell<DecodeAttemptStateV1>,
    grants: Cell<usize>,
    refuse_after: usize,
    aborted: Cell<bool>,
    identities: Cell<usize>,
}

impl TestAdmission {
    fn new(refuse_after: usize) -> Self {
        Self {
            attempts: RefCell::new(DecodeAttemptStateV1::default()),
            grants: Cell::new(0),
            refuse_after,
            aborted: Cell::new(false),
            identities: Cell::new(0),
        }
    }

    fn grant(&self) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.get() || self.grants.get() >= self.refuse_after {
            return Err(DecodeBackingRefusalV1);
        }
        self.grants.set(
            self.grants
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
        self.attempts.borrow_mut().abort_v1();
    }

    fn enter_decode_field_v1(
        &self,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        let mut attempt = self.attempts.borrow_mut();
        if !attempt.is_reading_v1() {
            attempt.begin_decode_v1()?;
        }
        attempt.enter_field_v1(kind, maximum)
    }

    fn leave_decode_field_v1(
        &self,
        token: DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.attempts.borrow_mut().leave_field_v1(token)
    }

    fn reserve_text(
        &self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.grant()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)
    }

    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.grant()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)
    }

    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.grant()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| DecodeBackingRefusalV1)
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        self.grant()?;
        Ok(source.into())
    }

    fn adopt_parser_text<E>(
        &self,
        _source: &mut Option<String>,
        _validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
        Err(DecodeBackingRefusalV1.into())
    }

    fn copy_boxed_parser_text<E>(
        &self,
        _source: &mut Option<String>,
        _validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, crate::ParserTextInputErrorV1<E>> {
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
        self.identities
            .set(self.identities.get().checked_add(1).ok_or_else(|| {
                <D::Error as serde::de::Error>::custom("fixture repo identity count overflow")
            })?);
        RepoId::deserialize(decoder)
    }

    fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
        &self,
        decoder: D,
    ) -> Result<RevisionId, D::Error> {
        self.identities
            .set(self.identities.get().checked_add(1).ok_or_else(|| {
                <D::Error as serde::de::Error>::custom("fixture revision identity count overflow")
            })?);
        RevisionId::deserialize(decoder)
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn wire() -> String {
    format!(
        r#"{{"repo_id":"r","revision_id":"v","generation":1,"manifest_digest":null,"batch_digest":"batch","commits":[{{"wire_version":1,"sha":"{SHA}","parents":["{SHA}"],"author_time_ms":1,"committer_time_ms":2,"applied_at_ms":3,"author":"a","author_name":null,"author_email":null,"committer":"c","committer_name":null,"committer_email":null,"message":"m","is_merge":false,"tags":["t"]}}],"refs":[{{"Upsert":{{"name":"main","sha":"{SHA}"}}}}],"tags":[{{"Delete":{{"name":"old"}}}}],"diff_hunks":[{{"commit_sha":"{SHA}","file_path":"src/a.rs","record":{{"wire_version":1,"hunk_header":"@@","side":"after","added_text":"new","removed_text":"old","touched_text":"both","byte_start":0,"byte_end":4}}}}]}}"#
    )
}

fn decode(
    wire: &str,
    admission: &TestAdmission,
    limits: HistoryDecodeLimitsV1,
) -> Result<HistoryIngestBatch, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    HistoryIngestBatch::deserialize_seed_with_admission_v1(admission, limits)
        .deserialize(&mut decoder)
}

fn limits() -> HistoryDecodeLimitsV1 {
    HistoryDecodeLimitsV1 {
        text_bytes: 256,
        collection_items: 4,
    }
}

#[test]
fn nested_history_matches_fixed_ordinary_wire() -> TestResult {
    let wire = wire();
    let owner = TestAdmission::new(usize::MAX);
    let admitted = decode(&wire, &owner, limits())?;
    let ordinary: HistoryIngestBatch = serde_json::from_str(&wire)?;
    if admitted != ordinary || owner.identities.get() != 2 || owner.grants.get() < 10 {
        return Err("admitted History3 diverged or skipped nested grants".into());
    }
    Ok(())
}

#[test]
fn nested_text_and_collection_limits_refuse() {
    let wire = wire();
    let owner = TestAdmission::new(usize::MAX);
    let mut short_text = limits();
    short_text.text_bytes = 1;
    assert!(decode(&wire, &owner, short_text).is_err());
    assert!(owner.aborted.get());

    let owner = TestAdmission::new(usize::MAX);
    let mut no_items = limits();
    no_items.collection_items = 0;
    assert!(decode(&wire, &owner, no_items).is_err());
    assert!(owner.aborted.get());

    // The outer digest receives one grant, then the nested parent vector is
    // denied before its backing can grow.
    let owner = TestAdmission::new(1);
    assert!(decode(&wire, &owner, limits()).is_err());
    assert!(owner.aborted.get());
    assert_eq!(owner.identities.get(), 2);
    assert_eq!(owner.grants.get(), 1);
}

#[test]
fn nested_unknown_duplicate_nullable_and_invalid_sha_refuse() {
    let wire = wire();
    for malformed in [
        wire.replace("\"name\":\"main\"", "\"name\":\"main\",\"unknown\":1"),
        wire.replace("\"name\":\"main\"", "\"name\":\"main\",\"name\":\"again\""),
        wire.replace("\"name\":\"main\"", "\"name\":null"),
        wire.replace(SHA, "bad"),
    ] {
        let owner = TestAdmission::new(usize::MAX);
        assert!(serde_json::from_str::<HistoryIngestBatch>(&malformed).is_err());
        assert!(decode(&malformed, &owner, limits()).is_err());
        assert!(owner.aborted.get());
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed ordinary/admitted wire and refusal assertions are the independent test oracle"
)]
fn recency_batch_matches_ordinary_and_refuses_entry_growth() -> TestResult {
    let wire = r#"{"repo_id":"r","revision_id":"v","generation":1,"batch_digest":"b","entries":[{"source_repo_id":"other","latest_committer_time_ms":42}]}"#;
    let ordinary: RepoCommitRecencyIngestBatch = serde_json::from_str(wire)?;
    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let admitted =
        RepoCommitRecencyIngestBatch::deserialize_seed_with_admission_v1(&owner, limits())
            .deserialize(&mut decoder)?;
    assert_eq!(admitted, ordinary);
    assert_eq!(owner.identities.get(), 3);
    assert!(owner.grants.get() >= 2);

    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let mut limits = limits();
    limits.collection_items = 0;
    assert!(
        RepoCommitRecencyIngestBatch::deserialize_seed_with_admission_v1(&owner, limits)
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(owner.aborted.get());
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed ordinary/admitted wire and nested refusal assertions are the independent test oracle"
)]
fn contributor_identity_nullable_path_and_nested_refusals() -> TestResult {
    let wire = r#"{"repo_id":"r","revision_id":"v","generation":1,"batch_digest":"b","entries":[{"source_repo_id":"other","repo_relative_path":"src/a.rs","contributors":[{"canonical":"person","name":null,"email":"x@y"}]}]}"#;
    let ordinary: FileContributorIngestBatch = serde_json::from_str(wire)?;
    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let admitted = FileContributorIngestBatch::deserialize_seed_with_admission_v1(&owner, limits())
        .deserialize(&mut decoder)?;
    assert_eq!(admitted, ordinary);
    assert_eq!(owner.identities.get(), 3);
    assert!(owner.grants.get() >= 4);

    let mut short_text = limits();
    short_text.text_bytes = 3;
    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(wire);
    assert!(
        FileContributorIngestBatch::deserialize_seed_with_admission_v1(&owner, short_text)
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(owner.aborted.get());

    let mut no_items = limits();
    no_items.collection_items = 0;
    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(wire);
    assert!(
        FileContributorIngestBatch::deserialize_seed_with_admission_v1(&owner, no_items)
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(owner.aborted.get());

    let duplicate = wire.replace(
        "\"canonical\":\"person\"",
        "\"canonical\":\"person\",\"canonical\":\"again\"",
    );
    assert!(serde_json::from_str::<FileContributorIngestBatch>(&duplicate).is_err());
    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(&duplicate);
    assert!(
        FileContributorIngestBatch::deserialize_seed_with_admission_v1(&owner, limits())
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(owner.aborted.get());
    Ok(())
}
