use std::cell::Cell;

use quanta_index_contract_base::{
    DecodeBackingAdmissionV1, DecodeBackingRefusalV1, SourceIdentityDecodeAdmissionV1,
};
use serde::{Deserialize, Deserializer, de::DeserializeSeed};

use super::ChunkRecord;
use crate::{RepoId, RevisionId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Semantic policy fixture only. In particular, its RepoId method does not
// issue native identity DATA or an Original allocation grant.
struct TestAdmission {
    aborted: Cell<bool>,
    text_reservations: Cell<usize>,
    boxed_copies: Cell<usize>,
    repo_calls: Cell<usize>,
    refuse_text_after: usize,
    refuse_box_after: usize,
    attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
}

impl TestAdmission {
    fn new() -> Self {
        Self {
            aborted: Cell::new(false),
            text_reservations: Cell::new(0),
            boxed_copies: Cell::new(0),
            repo_calls: Cell::new(0),
            refuse_text_after: usize::MAX,
            refuse_box_after: usize::MAX,
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
        if self.aborted.get() || self.text_reservations.get() >= self.refuse_text_after {
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
        _destination: &mut Vec<T>,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        if self.aborted.get() || self.boxed_copies.get() >= self.refuse_box_after {
            return Err(DecodeBackingRefusalV1);
        }
        let copy = source.into();
        self.boxed_copies
            .set(self.boxed_copies.get().saturating_add(1));
        Ok(copy)
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
        self.repo_calls.set(self.repo_calls.get().saturating_add(1));
        RepoId::deserialize(decoder)
    }

    fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
        &self,
        _decoder: D,
    ) -> Result<RevisionId, D::Error> {
        Err(serde::de::Error::custom(
            "ChunkRecord has no revision-id field",
        ))
    }
}

fn decode_admitted(
    wire: &str,
    admission: &TestAdmission,
    max_text_bytes: usize,
) -> Result<ChunkRecord, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let result = ChunkRecord::deserialize_seed_with_admission_v1(admission, max_text_bytes)
        .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(result)
}

const FULL: &str = r#"{"chunk_id":"chunk:fixed","repo_relative_path":"src/main.rs","language":"rust","start_byte":1,"end_byte":12,"start_line":2,"end_line":3,"text":"fn main() {}","structural":{"variant_tag":"node","structural_kind_tag":"function","structural_pattern_kind":"definition","structural_name":"main","structural_scope":"crate","structural_matched_node":"fn main"},"parent_chunk_id":"chunk:parent","source_repo_id":"repo:source"}"#;
const NO_OPTIONAL_VALUES: &str = r#"{"chunk_id":"chunk:fixed","repo_relative_path":"src/main.rs","language":"rust","start_byte":1,"end_byte":12,"start_line":2,"end_line":3,"text":"fn main() {}","structural":null,"parent_chunk_id":null,"source_repo_id":null}"#;

#[test]
fn admitted_chunk_matches_existing_wire_with_all_optional_values() -> TestResult {
    let admission = TestAdmission::new();
    let admitted = decode_admitted(FULL, &admission, 64)?;
    let ordinary: ChunkRecord = serde_json::from_str(FULL)?;
    if admitted != ordinary
        || admitted.text.as_ref() != "fn main() {}"
        || admitted.source_repo_id.as_ref().map(RepoId::as_str) != Some("repo:source")
    {
        return Err("admitted ChunkRecord changed the fixed wire's values".into());
    }
    // ChunkId, RepoRelativePath and parent ChunkId are String-backed.
    // LanguageCode, text and six structural leaves are Box<str>-backed.
    if admission.text_reservations.get() != 3
        || admission.boxed_copies.get() != 8
        || admission.repo_calls.get() != 1
        || admission.aborted.get()
    {
        return Err(format!(
            "FULL backing dispatch: text={}, boxes={}, repo={}, aborted={}",
            admission.text_reservations.get(),
            admission.boxed_copies.get(),
            admission.repo_calls.get(),
            admission.aborted.get(),
        )
        .into());
    }
    Ok(())
}

#[test]
fn admitted_chunk_preserves_explicit_none_optionals() -> TestResult {
    let admission = TestAdmission::new();
    let admitted = decode_admitted(NO_OPTIONAL_VALUES, &admission, 64)?;
    let ordinary: ChunkRecord = serde_json::from_str(NO_OPTIONAL_VALUES)?;
    if admitted != ordinary
        || admitted.structural.is_some()
        || admitted.parent_chunk_id.is_some()
        || admitted.source_repo_id.is_some()
    {
        return Err("explicit null optionals changed their accepted wire".into());
    }
    // The two required String IDs remain; language and text retain boxes.
    if admission.text_reservations.get() != 2
        || admission.boxed_copies.get() != 2
        || admission.repo_calls.get() != 0
        || admission.aborted.get()
    {
        return Err(format!(
            "NONE backing dispatch: text={}, boxes={}, repo={}, aborted={}",
            admission.text_reservations.get(),
            admission.boxed_copies.get(),
            admission.repo_calls.get(),
            admission.aborted.get(),
        )
        .into());
    }
    Ok(())
}

#[test]
fn admitted_chunk_rejects_missing_duplicate_unknown_and_wrong_type() -> TestResult {
    let Some(prefix) = FULL.strip_suffix('}') else {
        return Err("fixed ChunkRecord wire lost its closing brace".into());
    };
    let duplicate = format!("{prefix},\"chunk_id\":\"chunk:other\"}}");
    let missing = FULL.replace(",\"source_repo_id\":\"repo:source\"", "");
    let unknown = FULL.replace("\"chunk_id\":", "\"unexpected\":0,\"chunk_id\":");
    let wrong_type = FULL.replace("\"start_byte\":1", "\"start_byte\":\"one\"");
    for wire in [duplicate, missing, unknown, wrong_type] {
        let admission = TestAdmission::new();
        if decode_admitted(&wire, &admission, 64).is_ok() || !admission.aborted.get() {
            return Err("malformed ChunkRecord escaped the shared visitor's refusal".into());
        }
    }
    Ok(())
}

#[test]
fn admitted_chunk_denial_and_bound_abort_before_a_bare_record_escapes() -> TestResult {
    let mut limited = TestAdmission::new();
    limited.refuse_text_after = 0;
    if decode_admitted(FULL, &limited, 64).is_ok()
        || !limited.aborted.get()
        || limited.boxed_copies.get() != 0
    {
        return Err("text admission refusal produced a record or copied its text".into());
    }

    let mut box_limited = TestAdmission::new();
    box_limited.refuse_box_after = 1;
    if decode_admitted(FULL, &box_limited, 64).is_ok()
        || !box_limited.aborted.get()
        || box_limited.boxed_copies.get() != 1
    {
        return Err("boxed text refusal failed to abort a partial ChunkRecord".into());
    }

    let bounded = TestAdmission::new();
    if decode_admitted(FULL, &bounded, 4).is_ok()
        || !bounded.aborted.get()
        || bounded.text_reservations.get() != 0
    {
        return Err("text length bound was checked after backing birth".into());
    }
    Ok(())
}
