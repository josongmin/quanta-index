use std::cell::Cell;

use quanta_index_contract_base::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};
use serde::de::DeserializeSeed;

use super::SymbolRecord;

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Semantic dispatch fixture only; this is not a physical allocation grant.
struct TestAdmission {
    aborted: Cell<bool>,
    text_reservations: Cell<usize>,
    boxed_copies: Cell<usize>,
    refuse_box_after: usize,
    attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
}

impl TestAdmission {
    fn new(refuse_box_after: usize) -> Self {
        Self {
            aborted: Cell::new(false),
            text_reservations: Cell::new(0),
            boxed_copies: Cell::new(0),
            refuse_box_after,
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
        if self.aborted.get() {
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
        self.boxed_copies
            .set(self.boxed_copies.get().saturating_add(1));
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

fn decode_admitted(
    wire: &str,
    admission: &TestAdmission,
    max_text_bytes: usize,
) -> Result<SymbolRecord, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let result = SymbolRecord::deserialize_seed_with_admission_v1(admission, max_text_bytes)
        .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(result)
}

const FULL: &str = r#"{"symbol_id":"symbol:fixed","repo_relative_path":"src/main.rs","language":"rust","symbol_kind":"function","symbol_kind_family":"Callable","local_name":"run","qualified_name":"crate::run","signature":"fn run()","visibility":"public","definition_span":{"path":"src/main.rs","byte_start":4,"byte_end":12,"line_start":1,"line_end":1},"container_qualified_name":"crate","relationship":"Def"}"#;
const NULL_OPTIONALS: &str = r#"{"symbol_id":"symbol:fixed","repo_relative_path":"src/main.rs","language":"rust","symbol_kind":"function","symbol_kind_family":null,"local_name":"run","qualified_name":"crate::run","signature":null,"visibility":null,"definition_span":{"path":"src/main.rs","byte_start":4,"byte_end":12,"line_start":1,"line_end":1},"container_qualified_name":null,"relationship":"Def"}"#;

#[test]
fn admitted_symbol_matches_fixed_existing_wire_and_all_backing_births() -> TestResult {
    let admission = TestAdmission::new(usize::MAX);
    let admitted = decode_admitted(FULL, &admission, 64)?;
    let ordinary: SymbolRecord = serde_json::from_str(FULL)?;
    if admitted != ordinary
        || admitted.definition_span.path.as_ref() != "src/main.rs"
        || admitted.signature.as_deref() != Some("fn run()")
    {
        return Err("admitted SymbolRecord changed the fixed wire values".into());
    }
    // SymbolId and RepoRelativePath are String-backed. LanguageCode, kind,
    // local/qualified names, signature, span path and container are boxed.
    if admission.text_reservations.get() != 2
        || admission.boxed_copies.get() != 7
        || admission.aborted.get()
    {
        return Err(format!(
            "FULL backing dispatch: text={}, boxes={}, aborted={}",
            admission.text_reservations.get(),
            admission.boxed_copies.get(),
            admission.aborted.get()
        )
        .into());
    }
    Ok(())
}

#[test]
fn admitted_symbol_preserves_required_explicit_nulls() -> TestResult {
    let admission = TestAdmission::new(usize::MAX);
    let admitted = decode_admitted(NULL_OPTIONALS, &admission, 64)?;
    let ordinary: SymbolRecord = serde_json::from_str(NULL_OPTIONALS)?;
    if admitted != ordinary
        || admitted.signature.is_some()
        || admitted.symbol_kind_family.is_some()
        || admitted.visibility.is_some()
        || admitted.container_qualified_name.is_some()
    {
        return Err("explicit null fields changed the accepted SymbolRecord wire".into());
    }
    if admission.text_reservations.get() != 2
        || admission.boxed_copies.get() != 5
        || admission.aborted.get()
    {
        return Err(format!(
            "NULL backing dispatch: text={}, boxes={}, aborted={}",
            admission.text_reservations.get(),
            admission.boxed_copies.get(),
            admission.aborted.get()
        )
        .into());
    }
    Ok(())
}

#[test]
fn admitted_symbol_rejects_missing_duplicate_unknown_and_partial_wire() {
    let missing = FULL.replace(",\"relationship\":\"Def\"", "");
    let missing_optional = FULL.replace("\"signature\":\"fn run()\",", "");
    let duplicate = FULL.replace(
        "\"local_name\":\"run\"",
        "\"local_name\":\"run\",\"local_name\":\"again\"",
    );
    let unknown = FULL.replace(
        "\"relationship\":\"Def\"",
        "\"unknown\":0,\"relationship\":\"Def\"",
    );
    let nested_duplicate = FULL.replace("\"byte_start\":4", "\"byte_start\":4,\"byte_start\":5");
    let nested_unknown = FULL.replace("\"byte_start\":4", "\"unexpected\":0,\"byte_start\":4");
    let invalid_kind = FULL.replace(
        "\"symbol_kind\":\"function\"",
        "\"symbol_kind\":\"unknown\"",
    );
    let partial = "{\"symbol_id\":\"symbol:fixed\",\"definition_span\":{\"path\":\"src/main.rs\"";
    for wire in [
        &missing,
        &missing_optional,
        &duplicate,
        &unknown,
        &nested_duplicate,
        &nested_unknown,
        &invalid_kind,
        partial,
    ] {
        let admission = TestAdmission::new(usize::MAX);
        assert!(
            decode_admitted(wire, &admission, 64).is_err() && admission.aborted.get(),
            "invalid or partial SymbolRecord admitted: {wire}"
        );
    }
}

#[test]
fn admitted_symbol_denial_aborts_before_nested_span_completion() {
    let admission = TestAdmission::new(5);
    assert!(
        decode_admitted(FULL, &admission, 64).is_err()
            && admission.aborted.get()
            && admission.boxed_copies.get() == 5,
        "nested span backing refusal was not terminal"
    );
    let short = TestAdmission::new(usize::MAX);
    assert!(
        decode_admitted(FULL, &short, 3).is_err() && short.aborted.get(),
        "bounded symbol text was accepted past its maximum"
    );
}
