use std::cell::Cell;

use quanta_index_contract_base::{
    AdmittedSequenceSeedV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1, RepoId, RevisionId,
    SourceIdentityDecodeAdmissionV1,
};
use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeSeed,
        value::{Error, SeqDeserializer},
    },
};

use super::{CorpusDecodeLimitsV1, ScalarSeed, SearchCorpusIngestBatch};
use core::marker::PhantomData;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ZERO_DIGEST: &str = "[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]";

// Semantic fixture only. Identity decoding here does not issue Original/NFC DATA.
struct TestAdmission {
    allocations: Cell<usize>,
    identity_calls: Cell<usize>,
    aborted: Cell<bool>,
    refuse_after: usize,
    attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
}

impl TestAdmission {
    fn new(refuse_after: usize) -> Self {
        Self {
            allocations: Cell::new(0),
            identity_calls: Cell::new(0),
            aborted: Cell::new(false),
            refuse_after,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        }
    }

    fn checkpoint(&self) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.get() || self.allocations.get() >= self.refuse_after {
            return Err(DecodeBackingRefusalV1);
        }
        self.allocations
            .set(self.allocations.get().saturating_add(1));
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
        kind: quanta_index_contract_base::DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<quanta_index_contract_base::DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        let mut attempt = self.attempt.borrow_mut();
        if !attempt.is_reading_v1() {
            attempt.begin_decode_v1()?;
        }
        attempt.enter_field_v1(kind, maximum)
    }

    fn leave_decode_field_v1(
        &self,
        token: quanta_index_contract_base::DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.attempt.borrow_mut().leave_field_v1(token)
    }

    fn reserve_text(
        &self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.checkpoint()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_error| DecodeBackingRefusalV1)
    }

    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.checkpoint()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_error| DecodeBackingRefusalV1)
    }

    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.checkpoint()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_error| DecodeBackingRefusalV1)
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        self.checkpoint()?;
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
        self.identity_calls
            .set(self.identity_calls.get().saturating_add(1));
        RepoId::deserialize(decoder)
    }

    fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
        &self,
        decoder: D,
    ) -> Result<RevisionId, D::Error> {
        self.identity_calls
            .set(self.identity_calls.get().saturating_add(1));
        RevisionId::deserialize(decoder)
    }
}

fn limits() -> CorpusDecodeLimitsV1 {
    CorpusDecodeLimitsV1 {
        text_bytes: 64,
        source_bytes: 8,
        bundle_bytes: 8,
        collection_items: 2,
    }
}

fn root_json(replace_scopes: &str, bundle_payload: &str) -> String {
    format!(
        "{{\"source_event\":{{\"stream_id\":\"s\",\"event_id\":\"e\",\"expected_base_event_id\":null,\"payload_sha256\":{ZERO_DIGEST}}},\"repo_id\":\"r\",\"revision_id\":\"v\",\"generation\":1,\"base_generation\":null,\"manifest_digest\":\"m\",\"batch_digest\":\"b\",\"mode\":\"ReplaceGeneration\",\"bundle_payload\":{bundle_payload},\"clear_surfaces\":[],\"replace_scopes\":{replace_scopes},\"tombstone_scopes\":[],\"semantic_replace_scopes\":[],\"semantic_tombstone_scopes\":[],\"seal\":true}}"
    )
}

fn replacement_json() -> String {
    format!(
        "[{{\"coverage\":{{\"source\":{{\"file\":{{\"source_repo_id\":\"r\",\"repo_relative_path\":\"a.rs\"}},\"revision_id\":\"v\",\"source_sha256\":{ZERO_DIGEST}}},\"language\":\"rust\",\"producer_policy_sha256\":{ZERO_DIGEST},\"symbol_name_source_policy\":\"unspecified\",\"unit_set_sha256\":{ZERO_DIGEST},\"text_admitted\":true,\"symbols\":{{\"state\":\"not_requested\"}}}},\"source_bytes\":[1,2],\"chunks\":[],\"symbols\":[]}}]"
    )
}

fn decode_root(
    source: &str,
    admission: &TestAdmission,
    limits: CorpusDecodeLimitsV1,
) -> Result<SearchCorpusIngestBatch, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(source);
    SearchCorpusIngestBatch::deserialize_seed_with_admission_v1(admission, limits)
        .deserialize(&mut decoder)
}

#[test]
fn fixed_root_wire_preserves_existing_dto_and_nullable_bundle() -> TestResult {
    let json = root_json("[]", "null");
    let owner = TestAdmission::new(usize::MAX);
    let admitted = decode_root(&json, &owner, limits())?;
    let ordinary: SearchCorpusIngestBatch = serde_json::from_str(&json)?;
    if admitted != ordinary
        || admitted.bundle_payload.is_some()
        || admitted.source_event.stream_id != "s"
        || owner.identity_calls.get() != 2
        || owner.allocations.get() != 4
    {
        return Err("fixed root wire or admitted identities changed".into());
    }
    Ok(())
}

#[test]
fn nested_replacement_and_bundle_limits_refuse_before_destination_growth() -> TestResult {
    let replacement = replacement_json();
    let json = root_json(&replacement, "[1,2]");
    let owner = TestAdmission::new(usize::MAX);
    let admitted = decode_root(&json, &owner, limits())?;
    let ordinary: SearchCorpusIngestBatch = serde_json::from_str(&json)?;
    if admitted != ordinary
        || admitted.replace_scopes.len() != 1
        || admitted
            .replace_scopes
            .first()
            .map(|scope| scope.source_bytes.as_slice())
            != Some(&[1, 2][..])
        || admitted.bundle_payload.as_deref() != Some(&[1, 2][..])
    {
        return Err("nested replacement or integer-array bundle changed".into());
    }

    let mut no_source = limits();
    no_source.source_bytes = 1;
    let refused_source = TestAdmission::new(usize::MAX);
    if decode_root(&json, &refused_source, no_source).is_ok() || !refused_source.aborted.get() {
        return Err("oversized source bytes entered the returned DTO".into());
    }

    let mut no_collection = limits();
    no_collection.collection_items = 0;
    let refused_collection = TestAdmission::new(usize::MAX);
    if decode_root(&json, &refused_collection, no_collection).is_ok()
        || !refused_collection.aborted.get()
    {
        return Err("collection over-limit reached a child DTO".into());
    }

    let mut no_bundle = limits();
    no_bundle.bundle_bytes = 1;
    let refused_bundle = TestAdmission::new(usize::MAX);
    if decode_root(&json, &refused_bundle, no_bundle).is_ok() || !refused_bundle.aborted.get() {
        return Err("oversized bundle was admitted".into());
    }
    Ok(())
}

#[test]
fn bundle_writer_array_and_reader_byte_string_preserve_existing_semantics() -> TestResult {
    // The writer emits an integer array. The existing Ciborium Vec<u8> reader
    // also accepts byte strings through BytesAccess; admitted decode uses the
    // same deserialize_seq visitor and bounded per-element admission.
    const BYTE_STRING_WIRE: &[u8] = b"\x42\x01\x02";
    let ordinary_bytes: Vec<u8> = ciborium::from_reader(BYTE_STRING_WIRE)?;
    if ordinary_bytes != [1, 2] {
        return Err("fixed byte-string CBOR fixture changed".into());
    }
    let owner = TestAdmission::new(usize::MAX);
    let admitted = AdmittedSequenceSeedV1::new(&owner, 2, || ScalarSeed::<u8>(PhantomData))
        .deserialize(SeqDeserializer::<_, Error>::new(ordinary_bytes.into_iter()))?;
    if admitted != [1, 2] || owner.allocations.get() != 2 || owner.aborted.get() {
        return Err("bounded admitted bundle sequence changed".into());
    }
    Ok(())
}

#[test]
fn admission_refusal_and_nested_malformed_fields_abort_exact_attempt() -> TestResult {
    let json = root_json("[]", "null");
    let refusal = TestAdmission::new(1);
    if decode_root(&json, &refusal, limits()).is_ok()
        || !refusal.aborted.get()
        || refusal.allocations.get() != 1
        || refusal.identity_calls.get() != 0
    {
        return Err("early root backing refusal decoded later identities".into());
    }

    let malformed_replacement =
        "[{\"coverage\":{},\"source_bytes\":[1],\"chunks\":[],\"symbols\":[]}]";
    let malformed = root_json(malformed_replacement, "null");
    let owner = TestAdmission::new(usize::MAX);
    if decode_root(&malformed, &owner, limits()).is_ok()
        || serde_json::from_str::<SearchCorpusIngestBatch>(&malformed).is_ok()
        || !owner.aborted.get()
    {
        return Err("nested malformed coverage was accepted or not terminal".into());
    }
    Ok(())
}
