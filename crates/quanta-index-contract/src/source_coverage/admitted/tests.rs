use std::cell::Cell;

use quanta_index_contract_base::{
    DecodeBackingAdmissionV1, DecodeBackingRefusalV1, RepoId, RevisionId,
    SourceIdentityDecodeAdmissionV1,
};
use serde::{Deserialize, Deserializer, de::DeserializeSeed};

use super::{SourceFileCoverage, SourcePublicationEvent, SymbolCoverage};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ZERO_DIGEST: &str = "[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]";
const EVENT_CBOR_PREFIX: &[u8] = b"\xa4\x69stream_id\x61s\x68event_id\x61e\x76expected_base_event_id\xf6\x6epayload_sha256\x98\x20";

// Semantic fixture only; its identity methods do not mint Original/NFC DATA.
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

    fn next_allocation(&self) -> Result<(), DecodeBackingRefusalV1> {
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
        self.next_allocation()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_error| DecodeBackingRefusalV1)
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
        _destination: &mut Vec<T>,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        self.next_allocation()?;
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

fn decode_event(
    source: &str,
    owner: &TestAdmission,
) -> Result<SourcePublicationEvent, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(source);
    SourcePublicationEvent::deserialize_seed_with_admission_v1(owner, 512).deserialize(&mut decoder)
}

fn fixed_event_json(base: &str) -> String {
    format!(
        "{{\"stream_id\":\"s\",\"event_id\":\"e\",\"expected_base_event_id\":{base},\"payload_sha256\":{ZERO_DIGEST}}}"
    )
}

fn fixed_coverage_json() -> String {
    format!(
        "{{\"source\":{{\"file\":{{\"source_repo_id\":\"r\",\"repo_relative_path\":\"a.rs\"}},\"revision_id\":\"v\",\"source_sha256\":{ZERO_DIGEST}}},\"language\":\"rust\",\"producer_policy_sha256\":{ZERO_DIGEST},\"symbol_name_source_policy\":\"unspecified\",\"unit_set_sha256\":{ZERO_DIGEST},\"text_admitted\":true,\"symbols\":{{\"state\":\"complete\",\"symbol_count\":0}}}}"
    )
}

#[test]
fn fixed_json_and_cbor_event_preserve_public_value_and_null_presence() -> TestResult {
    let json = fixed_event_json("null");
    let owner = TestAdmission::new(usize::MAX);
    let admitted = decode_event(&json, &owner)?;
    let ordinary: SourcePublicationEvent = serde_json::from_str(&json)?;
    if admitted != ordinary
        || admitted.stream_id != "s"
        || admitted.event_id != "e"
        || admitted.expected_base_event_id.is_some()
        || admitted.payload_sha256 != [0; 32]
        || owner.allocations.get() != 2
    {
        return Err("fixed JSON event admission changed public semantics".into());
    }

    let mut cbor = EVENT_CBOR_PREFIX.to_vec();
    cbor.extend_from_slice(&[0; 32]);
    let cbor_event: SourcePublicationEvent = ciborium::from_reader(cbor.as_slice())?;
    if cbor_event != admitted {
        return Err("fixed CBOR event changed public semantics".into());
    }
    Ok(())
}

#[test]
fn fixed_coverage_reuses_source_revision_and_language_admission() -> TestResult {
    let json = fixed_coverage_json();
    let owner = TestAdmission::new(usize::MAX);
    let mut decoder = serde_json::Deserializer::from_str(&json);
    let admitted = SourceFileCoverage::deserialize_seed_with_admission_v1(&owner, 4096)
        .deserialize(&mut decoder)?;
    let ordinary: SourceFileCoverage = serde_json::from_str(&json)?;
    if admitted != ordinary
        || admitted.source.file.repo_relative_path.as_str() != "a.rs"
        || admitted.symbols != (SymbolCoverage::Complete { symbol_count: 0 })
        || owner.identity_calls.get() != 2
        || owner.allocations.get() != 2
    {
        return Err("fixed coverage changed identity, language or symbol admission".into());
    }
    Ok(())
}

#[test]
fn partial_refusal_aborts_and_preserves_original_semantic_denial() -> TestResult {
    let json = fixed_coverage_json();
    let owner = TestAdmission::new(1);
    let mut decoder = serde_json::Deserializer::from_str(&json);
    let rejected = SourceFileCoverage::deserialize_seed_with_admission_v1(&owner, 4096)
        .deserialize(&mut decoder);
    if rejected.is_ok() || !owner.aborted.get() || owner.allocations.get() != 1 {
        return Err("coverage continued after second backing refusal".into());
    }

    let event_owner = TestAdmission::new(2);
    let event = fixed_event_json("\"base\"");
    if decode_event(&event, &event_owner).is_ok()
        || !event_owner.aborted.get()
        || event_owner.allocations.get() != 2
    {
        return Err("optional event token continued after backing refusal".into());
    }
    Ok(())
}

#[test]
fn event_byte_limit_and_closed_map_errors_do_not_mint_values() -> TestResult {
    let overlong = format!(
        "{{\"stream_id\":\"s\",\"event_id\":\"{}\",\"expected_base_event_id\":null,\"payload_sha256\":{ZERO_DIGEST}}}",
        "x".repeat(513)
    );
    let owner = TestAdmission::new(usize::MAX);
    if decode_event(&overlong, &owner).is_ok()
        || !owner.aborted.get()
        || owner.allocations.get() != 1
    {
        return Err("oversized event token allocated before refusal".into());
    }

    for invalid in [
        format!(
            "{{\"stream_id\":\"s\",\"stream_id\":\"s\",\"event_id\":\"e\",\"expected_base_event_id\":null,\"payload_sha256\":{ZERO_DIGEST}}}"
        ),
        format!("{{\"stream_id\":\"s\",\"event_id\":\"e\",\"payload_sha256\":{ZERO_DIGEST}}}"),
        format!(
            "{{\"stream_id\":\"s\",\"event_id\":\"e\",\"expected_base_event_id\":null,\"payload_sha256\":{ZERO_DIGEST},\"foreign\":0}}"
        ),
    ] {
        let admitted = TestAdmission::new(usize::MAX);
        if decode_event(&invalid, &admitted).is_ok()
            || serde_json::from_str::<SourcePublicationEvent>(&invalid).is_ok()
            || !admitted.aborted.get()
        {
            return Err("closed event map accepted duplicate, missing or unknown field".into());
        }
    }
    Ok(())
}
