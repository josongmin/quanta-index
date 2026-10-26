use std::cell::Cell;

use quanta_index_contract_base::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};
use serde::de::DeserializeSeed;

use super::{SemanticSourceRecordV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Semantic dispatch fixture, not an Original or physical allocation owner.
struct TestAdmission {
    aborted: Cell<bool>,
    text_reservations: Cell<usize>,
    sequence_reservations: Cell<usize>,
    refuse_after: usize,
    attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
}

impl TestAdmission {
    fn new(refuse_after: usize) -> Self {
        Self {
            aborted: Cell::new(false),
            text_reservations: Cell::new(0),
            sequence_reservations: Cell::new(0),
            refuse_after,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
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
        if self.aborted.get() || self.text_reservations.get() >= self.refuse_after {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_physical_failure| DecodeBackingRefusalV1)?;
        self.text_reservations
            .set(self.text_reservations.get().saturating_add(1));
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
            .map_err(|_physical_failure| DecodeBackingRefusalV1)?;
        self.sequence_reservations
            .set(self.sequence_reservations.get().saturating_add(1));
        Ok(())
    }

    fn copy_boxed_text(&self, _source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
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

fn decode_scope(
    wire: &str,
    admission: &TestAdmission,
) -> Result<SemanticSourceScopeKeyV1, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let value = SemanticSourceScopeKeyV1::deserialize_seed_with_admission_v1(admission, 64)
        .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

fn decode_record(
    wire: &str,
    admission: &TestAdmission,
    max_record_bytes: usize,
) -> Result<SemanticSourceRecordV1, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let value =
        SemanticSourceRecordV1::deserialize_seed_with_admission_v1(admission, 64, max_record_bytes)
            .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

fn decode_replace(
    wire: &str,
    admission: &TestAdmission,
    max_record_bytes: usize,
    max_records: usize,
) -> Result<SemanticSourceReplaceScopeV1, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let value = SemanticSourceReplaceScopeV1::deserialize_seed_with_admission_v1(
        admission,
        64,
        max_record_bytes,
        max_records,
    )
    .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

const SCOPE: &str = r#"{"corpus_kind":"SymbolCard","owner_kind":"Symbol","owner_id":"symbol-1"}"#;
const RECORD: &str = r#"{"record_id":"record-1","corpus_kind":"SymbolCard","owner_kind":"Symbol","owner_id":"symbol-1","source_doc_id":"doc-1","parent_owner_id":null,"repo_relative_path":"src/lib.rs","language":"rust","package":"crate","symbol_kind":"function","visibility":"pub","source_role":"CardText","generated":false,"capability_status":"Full","raw_fallback_reason":null,"authority_digest":"auth:sha256:1","render_policy_digest":"render:sha256:1","card_schema_version":1,"text":"pub fn demo() {}"}"#;
const REPLACE: &str = r#"{"scope":{"corpus_kind":"SymbolCard","owner_kind":"Symbol","owner_id":"symbol-1"},"scope_digest":"scope:semantic:1","sources":[{"record_id":"record-1","corpus_kind":"SymbolCard","owner_kind":"Symbol","owner_id":"symbol-1","source_doc_id":"doc-1","parent_owner_id":null,"repo_relative_path":"src/lib.rs","language":"rust","package":"crate","symbol_kind":"function","visibility":"pub","source_role":"CardText","generated":false,"capability_status":"Full","raw_fallback_reason":null,"authority_digest":"auth:sha256:1","render_policy_digest":"render:sha256:1","card_schema_version":1,"text":"pub fn demo() {}"}],"cluster_memberships":[]}"#;
const REPLACE_CLUSTER: &str = r#"{"scope":{"corpus_kind":"ClusterCard","owner_kind":"Symbol","owner_id":"cluster-1"},"scope_digest":"scope:cluster:1","sources":[],"cluster_memberships":[{"cluster_record_id":"cluster-1","authority_digest":"auth-1","members":["symbol-1"]}]}"#;

#[test]
fn admitted_scope_record_and_replace_match_fixed_public_wires() -> TestResult {
    let scope_admission = TestAdmission::new(usize::MAX);
    let scope = decode_scope(SCOPE, &scope_admission)?;
    if scope != serde_json::from_str::<SemanticSourceScopeKeyV1>(SCOPE)?
        || scope_admission.text_reservations.get() != 1
        || scope_admission.aborted.get()
    {
        return Err("scope admitted wire or owner_id birth differs".into());
    }

    let record_admission = TestAdmission::new(usize::MAX);
    let record = decode_record(RECORD, &record_admission, 64)?;
    if record != serde_json::from_str::<SemanticSourceRecordV1>(RECORD)?
        || record.text != "pub fn demo() {}"
        || record_admission.text_reservations.get() != 11
        || record_admission.aborted.get()
    {
        return Err(format!(
            "record wire/backing: text={}, aborted={}",
            record_admission.text_reservations.get(),
            record_admission.aborted.get(),
        )
        .into());
    }

    let replace_admission = TestAdmission::new(usize::MAX);
    let replace = decode_replace(REPLACE, &replace_admission, 64, 1)?;
    if replace != serde_json::from_str::<SemanticSourceReplaceScopeV1>(REPLACE)?
        || replace.sources.len() != 1
        || replace_admission.text_reservations.get() != 13
        || replace_admission.sequence_reservations.get() != 1
        || replace_admission.aborted.get()
    {
        return Err(format!(
            "replace wire/backing: text={}, sequence={}, aborted={}",
            replace_admission.text_reservations.get(),
            replace_admission.sequence_reservations.get(),
            replace_admission.aborted.get(),
        )
        .into());
    }
    Ok(())
}

#[test]
fn admitted_replace_reuses_bounded_cluster_membership_seed() -> TestResult {
    let admission = TestAdmission::new(usize::MAX);
    let admitted = decode_replace(REPLACE_CLUSTER, &admission, 64, 1)?;
    let ordinary: SemanticSourceReplaceScopeV1 = serde_json::from_str(REPLACE_CLUSTER)?;
    if admitted != ordinary
        || admission.text_reservations.get() != 5
        || admission.sequence_reservations.get() != 2
        || admission.aborted.get()
    {
        return Err(format!(
            "cluster wire/backing: text={}, sequence={}, aborted={}",
            admission.text_reservations.get(),
            admission.sequence_reservations.get(),
            admission.aborted.get(),
        )
        .into());
    }
    let limit = TestAdmission::new(usize::MAX);
    if decode_replace(REPLACE_CLUSTER, &limit, 64, 0).is_ok() || !limit.aborted.get() {
        return Err("cluster collection exceeded its count bound".into());
    }
    Ok(())
}

#[test]
fn admitted_semantic_source_refuses_bounds_and_partial_nested_inputs() {
    let short_text = TestAdmission::new(usize::MAX);
    assert!(
        decode_record(RECORD, &short_text, 4).is_err() && short_text.aborted.get(),
        "record text exceeded its separate byte bound"
    );

    let no_records = TestAdmission::new(usize::MAX);
    assert!(
        decode_replace(REPLACE, &no_records, 64, 0).is_err() && no_records.aborted.get(),
        "source collection exceeded its count bound"
    );

    let denial = TestAdmission::new(3);
    assert!(
        decode_replace(REPLACE, &denial, 64, 1).is_err()
            && denial.aborted.get()
            && denial.text_reservations.get() == 3,
        "nested text refusal did not terminate the shared attempt"
    );

    let truncated = "{\"scope\":{\"corpus_kind\":\"SymbolCard\"";
    let partial = TestAdmission::new(usize::MAX);
    assert!(
        decode_replace(truncated, &partial, 64, 1).is_err() && partial.aborted.get(),
        "partial nested scope did not abort"
    );
}

#[test]
fn admitted_semantic_source_preserves_required_and_closed_fields() {
    let absent_optional = RECORD.replace("\"parent_owner_id\":null,", "");
    let duplicate = RECORD.replace(
        "\"record_id\":\"record-1\"",
        "\"record_id\":\"record-1\",\"record_id\":\"record-2\"",
    );
    let invalid_role = RECORD.replace(
        "\"source_role\":\"CardText\"",
        "\"source_role\":\"RawFallbackText\"",
    );
    let unknown_key = REPLACE.replace(
        "\"scope_digest\":\"scope:semantic:1\"",
        "\"surprise\":0,\"scope_digest\":\"scope:semantic:1\"",
    );
    let duplicate_scope = REPLACE.replace(
        "\"scope_digest\":\"scope:semantic:1\"",
        "\"scope_digest\":\"scope:semantic:1\",\"scope_digest\":\"other\"",
    );
    for wire in [&absent_optional, &duplicate, &invalid_role] {
        let admission = TestAdmission::new(usize::MAX);
        assert!(
            decode_record(wire, &admission, 64).is_err() && admission.aborted.get(),
            "invalid record wire admitted: {wire}"
        );
    }
    for wire in [&unknown_key, &duplicate_scope] {
        let admission = TestAdmission::new(usize::MAX);
        assert!(
            decode_replace(wire, &admission, 64, 1).is_err() && admission.aborted.get(),
            "invalid replace wire admitted: {wire}"
        );
    }
}
