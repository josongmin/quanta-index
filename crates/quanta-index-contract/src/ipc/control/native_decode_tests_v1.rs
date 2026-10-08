use super::*;
use core::cell::Cell;
use quanta_index_contract_base::{
    NativeActivationTokenDataFailureV1, NativeActivationTokenDecodeAdmissionV1,
    NativeIdentityConstructionErrorV1, NativeIdentityDecodeAdmissionV1, NativeIdentityDecodeLoanV1,
    NativeNormalizationAdmissionV1, NativeNormalizationScratchDemandV1,
    NativeNormalizationScratchOwnerV1,
};
use std::rc::Rc;

struct Grant(Rc<Cell<u32>>);
impl Drop for Grant {
    fn drop(&mut self) {
        let (count, underflow) = self.0.get().overflowing_sub(1);
        assert!(!underflow, "live grant underflow");
        self.0.set(count);
    }
}
struct Funding {
    grants: [Option<Grant>; 3],
    alive: Rc<Cell<u32>>,
}
struct Normalizer<'a> {
    funding: &'a mut Funding,
    late: Option<Box<u8>>,
}
impl NativeNormalizationAdmissionV1 for Normalizer<'_> {
    type Error = Box<u8>;
    fn checkpoint_work_v1(&mut self, _: u64) -> Result<(), Self::Error> {
        Ok(())
    }
    fn native_birth_v1(
        &mut self,
        demand: NativeNormalizationScratchDemandV1,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error> {
        let slot = match demand.owner_v1 {
            NativeNormalizationScratchOwnerV1::Decomposition => &mut self.funding.grants[0],
            NativeNormalizationScratchOwnerV1::Recomposition => &mut self.funding.grants[1],
            NativeNormalizationScratchOwnerV1::Sort => &mut self.funding.grants[2],
        };
        let success = birth();
        if success {
            let grant = Grant(Rc::clone(&self.funding.alive));
            grant
                .0
                .set(grant.0.get().checked_add(1).ok_or_else(|| Box::new(249))?);
            *slot = Some(grant);
            if let Some(cause) = self.late.take() {
                return Err(cause);
            }
        }
        Ok(success)
    }
    fn release_scratch_v1(&mut self, _: NativeNormalizationScratchOwnerV1) {
        assert!(false, "external producer must retain all grants");
    }
}
#[derive(Default)]
struct Admission {
    borrowed: u32,
    owned: u32,
    work: u64,
    copies: usize,
    alive: Rc<Cell<u32>>,
    late_copy: Option<Box<u8>>,
    late_nfc: Option<Box<u8>>,
    work_limit: Option<u64>,
    work_cause: Option<Box<u8>>,
    skip_producer: bool,
}
macro_rules! identity_method_v1 {
    ($method:ident, $identity:ty, $counter:ident) => {
        fn $method(
            &mut self,
            loan: &mut NativeIdentityDecodeLoanV1<'_, '_, $identity, Box<u8>, Funding>,
        ) -> Result<(), NativeCorpusDecodeRefusalV1> {
            if self.skip_producer {
                return Ok(());
            }
            self.$counter = self
                .$counter
                .checked_add(1)
                .ok_or(NativeCorpusDecodeRefusalV1::Admission)?;
            let alive = Rc::clone(&self.alive);
            let late_nfc = self.late_nfc.take();
            let late_copy = self.late_copy.take();
            let copies = &mut self.copies;
            loan.with_funding_v1(|runner, funding| {
                let funding = funding.get_or_insert_with(|| Funding {
                    grants: [None, None, None],
                    alive,
                });
                let result = runner.try_fill_v1(
                    &mut Normalizer {
                        funding,
                        late: late_nfc,
                    },
                    |_, birth| {
                        *copies = copies.checked_add(1).ok_or_else(|| Box::new(250))?;
                        let success = birth();
                        if let Some(cause) = late_copy {
                            return Err(cause);
                        }
                        Ok(success)
                    },
                );
                result.map_err(|_status| match runner.failure_v1() {
                    Some(NativeIdentityConstructionErrorV1::Validation(cause)) => {
                        NativeCorpusDecodeRefusalV1::Identity(*cause)
                    }
                    _ => NativeCorpusDecodeRefusalV1::Admission,
                })
            })
            .map_err(|_status| NativeCorpusDecodeRefusalV1::InvalidNativeProducer)?
        }
    };
}
impl NativeIdentityDecodeAdmissionV1 for Admission {
    type OriginalError = Box<u8>;
    type Funding = Funding;
    type Error = NativeCorpusDecodeRefusalV1;
    identity_method_v1!(repo_id_from_owned_v1, RepoId, owned);
    identity_method_v1!(repo_id_from_borrowed_v1, RepoId, borrowed);
    identity_method_v1!(revision_id_from_owned_v1, RevisionId, owned);
    identity_method_v1!(revision_id_from_borrowed_v1, RevisionId, borrowed);
}
impl NativeActivationTokenDecodeAdmissionV1 for Admission {
    type ControlError = Box<u8>;
    type Error = NativeCorpusDecodeRefusalV1;
    fn consume_token_work_v1(&mut self, units: u64) -> Result<(), Self::ControlError> {
        self.consume_corpus_work_v1(units)
    }
    fn token_work_refusal_v1(&self) -> Self::Error {
        NativeCorpusDecodeRefusalV1::Admission
    }
    fn token_invalid_data_v1(
        &self,
        cause: NativeActivationTokenDataFailureV1,
        field: Option<&'static str>,
    ) -> Self::Error {
        let cause = match cause {
            NativeActivationTokenDataFailureV1::UnknownField => {
                NativeCorpusDataFailureV1::UnknownField
            }
            NativeActivationTokenDataFailureV1::DuplicateField => {
                NativeCorpusDataFailureV1::DuplicateField
            }
            NativeActivationTokenDataFailureV1::MissingField => {
                NativeCorpusDataFailureV1::MissingField
            }
            NativeActivationTokenDataFailureV1::Semantic => NativeCorpusDataFailureV1::Semantic,
        };
        Self::Error::InvalidData(cause, field)
    }
}
impl NativeCorpusDecodeAdmissionV1 for Admission {
    fn consume_corpus_work_v1(&mut self, units: u64) -> Result<(), Box<u8>> {
        let work = self.work.checked_add(units).ok_or_else(|| Box::new(251))?;
        if self.work_limit.is_some_and(|limit| work > limit) {
            return Err(self.work_cause.take().unwrap_or_else(|| Box::new(252)));
        }
        self.work = work;
        Ok(())
    }
    fn refuse_corpus_work_arithmetic_v1(&mut self) -> Box<u8> {
        Box::new(253)
    }
    fn admit_corpus_string_birth_v1(
        &mut self,
        _: usize,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Box<u8>> {
        self.copies = self.copies.checked_add(1).ok_or_else(|| Box::new(254))?;
        let success = birth();
        if let Some(cause) = self.late_copy.take() {
            return Err(cause);
        }
        Ok(success)
    }
    fn corpus_invalid_data_v1(
        &self,
        cause: NativeCorpusDataFailureV1,
        field: Option<&'static str>,
    ) -> NativeCorpusDecodeRefusalV1 {
        NativeCorpusDecodeRefusalV1::InvalidData(cause, field)
    }
}
fn decode_native(
    source: &str,
    admission: &mut Admission,
) -> Result<SearchCorpusActiveHeadV1, serde_json::Error> {
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut decoder = serde_json::Deserializer::from_str(source);
    SearchCorpusActiveHeadV1::native_decode_seed_v1(admission, &mut data)
        .deserialize(&mut decoder)?;
    decoder.end()?;
    let mut output = None;
    data.complete_into_slot_v1(&mut output)
        .map_err(serde::de::Error::custom)?;
    output.ok_or_else(|| serde::de::Error::custom("no head"))
}
#[test]
fn native_corpus_seed_uses_wire_bound_producers_and_canonical_validation() {
    let head = super::super::qi_act_01_tests::corpus_head(3, "digest-3", 1);
    let wire = serde_json::to_string(&head).expect("encode fixture");
    let mut admission = Admission::default();
    assert_eq!(
        decode_native(&wire, &mut admission).expect("decode head"),
        head
    );
    assert_eq!(admission.borrowed, 4);
    assert_eq!(admission.owned, 0);
    assert!(admission.work > 0);
    let invalid = wire.replacen("\"repo\"", "\"e\\u0301\"", 1);
    assert!(
        decode_native(&invalid, &mut Admission::default())
            .unwrap_err()
            .to_string()
            .contains("Identity(NonCanonical)")
    );
    let invalid = wire.replacen("\"Lexical\"", "\"Unknown\"", 1);
    assert!(
        decode_native(&invalid, &mut Admission::default())
            .unwrap_err()
            .to_string()
            .contains("InvalidData(Semantic, None)")
    );
    assert!(
        serde_json::from_str::<SearchPlaneTrackKind>("\"Unknown\"")
            .unwrap_err()
            .to_string()
            .contains("unknown variant `Unknown`")
    );
    assert!(
        decode_native("{\"unknown\":1}", &mut Admission::default())
            .unwrap_err()
            .to_string()
            .contains("UnknownField")
    );
}

#[test]
fn recursive_partial_and_full_noncopy_native_refusal_survive_unit_return() {
    let cause = Box::new(71_u8);
    let pointer = core::ptr::from_ref(cause.as_ref());
    let mut admission = Admission {
        late_copy: Some(cause),
        ..Admission::default()
    };
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut decoder = serde_json::Deserializer::from_str(
        r#"{"generation":{"lexical":{"manifest_digest":"retained-digest"}}}"#,
    );
    let error = SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
        .deserialize(&mut decoder)
        .unwrap_err();
    let digest = &data.state.generation.lexical.digest;
    assert_eq!(digest.backing.capacity(), "retained-digest".len());
    assert!(digest.backing.is_empty());
    let Some(NativeCorpusDecodeFailureV1::Copy(
        quanta_index_contract_base::NativeIdentityCopyErrorV1::Admission(cause),
    )) = data.failure_v1()
    else {
        assert!(false, "full copy cause missing");
        return;
    };
    assert_eq!(core::ptr::from_ref(cause.as_ref()), pointer);
    assert_eq!(**cause, 71);
    let work = admission.work;
    let copies = admission.copies;
    let polls = Rc::new(Cell::new(0));
    assert!(
        SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
            .deserialize(PollDeserializer(Rc::clone(&polls)))
            .is_err()
    );
    assert_eq!(polls.get(), 0);
    assert_eq!(admission.work, work);
    assert_eq!(admission.copies, copies);
    assert!(error.to_string().contains("Admission"));
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_err());
}
#[test]
fn normalization_late_failure_keeps_actual_funding_and_exact_box() {
    let input = format!("q{}", "\u{301}".repeat(20));
    let wire = format!(r#"{{"generation":{{"lexical":{{"repo_id":"{input}"}}}}}}"#);
    let cause = Box::new(73_u8);
    let pointer = core::ptr::from_ref(cause.as_ref());
    let mut admission = Admission {
        late_nfc: Some(cause),
        ..Admission::default()
    };
    let alive = Rc::clone(&admission.alive);
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut decoder = serde_json::Deserializer::from_str(&wire);
    let error = SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
        .deserialize(&mut decoder)
        .unwrap_err();
    assert!(alive.get() > 0);
    assert_eq!(admission.copies, 0);
    let Some(NativeCorpusDecodeFailureV1::Identity(
        NativeIdentityConstructionErrorV1::Normalization(
            quanta_index_contract_base::NativeNormalizationErrorV1::Admission(cause),
        ),
    )) = data.failure_v1()
    else {
        assert!(false, "full normalization cause missing");
        return;
    };
    assert_eq!(core::ptr::from_ref(cause.as_ref()), pointer);
    assert_eq!(**cause, 73);
    drop(admission);
    assert!(alive.get() > 0);
    drop(error);
    drop(decoder);
    assert!(alive.get() > 0);
    drop(data);
    assert_eq!(alive.get(), 0);
}
#[test]
fn owned_duplicate_key_keeps_first_subtree_and_both_keys() {
    let head = super::super::qi_act_01_tests::corpus_head(4, "owned-digest", 2);
    let mut value = serde_json::to_value(head).expect("fixture");
    let generation = value.get_mut("generation").expect("generation").take();
    let pointer = generation
        .get("lexical")
        .and_then(|v| v.get("manifest_digest"))
        .and_then(serde_json::Value::as_str)
        .expect("digest")
        .as_ptr();
    let entries = vec![
        (String::from("generation"), generation),
        (String::from("generation"), serde_json::Value::Null),
    ];
    let mut admission = Admission::default();
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let error = SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
        .deserialize(
            serde::de::value::MapDeserializer::<_, serde_json::Error>::new(entries.into_iter()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("DuplicateField"));
    assert_eq!(admission.owned, 4);
    assert_eq!(admission.borrowed, 0);
    assert_eq!(admission.copies, 0);
    assert_eq!(
        data.state.keys.owned.first().and_then(Option::as_deref),
        Some("generation")
    );
    assert_eq!(data.state.keys.pending.as_deref(), Some("generation"));
    let subtree = data
        .state
        .generation
        .output
        .as_ref()
        .expect("first subtree retained");
    assert_eq!(subtree.lexical.manifest_digest.as_ptr(), pointer);
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_err());
}
#[test]
fn final_work_refusal_retains_candidate_and_original_cause() {
    let head = super::super::qi_act_01_tests::corpus_head(8, "digest-8", 3);
    let wire = serde_json::to_string(&head).expect("fixture");
    let mut baseline = Admission::default();
    assert_eq!(decode_native(&wire, &mut baseline).expect("baseline"), head);
    let cause = Box::new(79_u8);
    let pointer = core::ptr::from_ref(cause.as_ref());
    let mut admission = Admission {
        work_limit: baseline.work.checked_sub(1),
        work_cause: Some(cause),
        ..Admission::default()
    };
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut decoder = serde_json::Deserializer::from_str(&wire);
    assert!(
        SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
            .deserialize(&mut decoder)
            .is_err()
    );
    assert_eq!(data.state.output.as_ref(), Some(&head));
    let Some(NativeCorpusDecodeFailureV1::Work(cause)) = data.failure_v1() else {
        assert!(false, "full work cause missing");
        return;
    };
    assert_eq!(core::ptr::from_ref(cause.as_ref()), pointer);
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_err());
}
#[test]
fn late_malformed_input_retains_completed_children_and_missing_input_never_publishes() {
    let head = super::super::qi_act_01_tests::corpus_head(9, "digest-9", 4);
    let mut value = serde_json::to_value(head).expect("fixture");
    let generation = value.get_mut("generation").expect("generation").take();
    let prefix = format!(
        r#"{{"generation":{generation},"activation_token":{{"activation_sequence":3,"root_incarnation":[7,7,"#
    );
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut admission = Admission::default();
    let mut decoder = serde_json::Deserializer::from_str(&prefix);
    assert!(
        SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
            .deserialize(&mut decoder)
            .is_err()
    );
    assert!(data.state.generation.output.is_some());
    assert!(data.state.token.output.is_none());
    assert!(!data.completed);
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_err());
}
#[test]
fn all_schema_levels_keep_owned_unknown_keys_and_reject_missing_fields() {
    for (wire, depth) in [
        (r#"{"unknown":1}"#, 0),
        (r#"{"generation":{"unknown":1}}"#, 1),
        (r#"{"generation":{"lexical":{"unknown":1}}}"#, 2),
        (r#"{"generation":{"semantic_content":{"unknown":1}}}"#, 3),
    ] {
        let value: serde_json::Value = serde_json::from_str(wire).expect("fixture");
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut admission = Admission::default();
        assert!(
            SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut data)
                .deserialize(value)
                .unwrap_err()
                .to_string()
                .contains("UnknownField")
        );
        let pending = match depth {
            0 => &data.state.keys.pending,
            1 => &data.state.generation.keys.pending,
            2 => &data.state.generation.lexical.keys.pending,
            _ => &data.state.generation.content.keys.pending,
        };
        assert_eq!(pending.as_deref(), Some("unknown"));
    }
    for wire in [
        "{}",
        r#"{"generation":{}}"#,
        r#"{"generation":{"lexical":{}}}"#,
        r#"{"generation":{"semantic_content":{}}}"#,
    ] {
        assert!(
            decode_native(wire, &mut Admission::default())
                .unwrap_err()
                .to_string()
                .contains("MissingField")
        );
    }
}
#[test]
fn many_occurrences_retain_distinct_funding_until_external_retirement() {
    let mut head = super::super::qi_act_01_tests::corpus_head(10, "digest-10", 5);
    let identity = format!("q{}", "\u{301}".repeat(20));
    head.generation.lexical.repo_id = RepoId::new(identity.as_str()).expect("canonical");
    head.generation.semantic.repo_id = RepoId::new(identity.as_str()).expect("canonical");
    head.generation.lexical.revision_id = RevisionId::new(identity.as_str()).expect("canonical");
    head.generation.semantic.revision_id = RevisionId::new(identity).expect("canonical");
    let wire = serde_json::to_string(&head).expect("fixture");
    // Collection backing belongs to the caller, admitted outside the unit
    // producer. This test is not a Runtime container-admission qualification.
    let mut data: Vec<_> = (0..37)
        .map(|_| NativeCorpusDecodeDataV1::new_v1())
        .collect();
    let mut admission = Admission::default();
    let alive = Rc::clone(&admission.alive);
    for occurrence in &mut data {
        let mut decoder = serde_json::Deserializer::from_str(&wire);
        SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, occurrence)
            .deserialize(&mut decoder)
            .expect("decode occurrence");
        decoder.end().expect("complete");
    }
    assert_eq!(admission.borrowed, 148);
    assert!(alive.get() > 8);
    drop(admission);
    assert!(alive.get() > 8);
    let mut occupied = Some(head.clone());
    let pointer = occupied
        .as_ref()
        .expect("occupied")
        .generation
        .lexical
        .repo_id
        .as_str()
        .as_ptr();
    let first = data.first_mut().expect("occurrence");
    assert!(first.complete_into_slot_v1(&mut occupied).is_err());
    assert_eq!(
        occupied
            .as_ref()
            .expect("occupied")
            .generation
            .lexical
            .repo_id
            .as_str()
            .as_ptr(),
        pointer
    );
    let mut published = None;
    first
        .complete_into_slot_v1(&mut published)
        .expect("pure move");
    assert_eq!(published.as_ref(), Some(&head));
    assert!(alive.get() > 8);
    drop(published);
    drop(data);
    assert_eq!(alive.get(), 0);
}
#[test]
fn identity_owned_input_keeps_exact_pointer_and_unit_data_blocks_decoder_repoll() {
    let input = format!("q{}", "\u{301}".repeat(20));
    let pointer = input.as_ptr();
    let mut admission = Admission::default();
    let mut data = quanta_index_contract_base::NativeIdentityDecodeDataV1::new_v1();
    RepoId::native_decode_seed_v1(&mut admission, &mut data)
        .deserialize(serde::de::value::StringDeserializer::<serde_json::Error>::new(input))
        .expect("owned identity");
    assert_eq!(admission.owned, 1);
    assert_eq!(admission.copies, 0);
    assert!(admission.alive.get() > 0);
    let polls = Rc::new(Cell::new(0));
    assert!(
        RepoId::native_decode_seed_v1(&mut admission, &mut data)
            .deserialize(PollDeserializer(Rc::clone(&polls)))
            .is_err()
    );
    assert_eq!(polls.get(), 0);
    assert_eq!(admission.owned, 1);
    let mut output = None;
    data.complete_into_slot_v1(&mut output).expect("pure move");
    assert_eq!(
        output.as_ref().expect("identity").as_str().as_ptr(),
        pointer
    );
    let alive = Rc::clone(&admission.alive);
    drop(admission);
    assert!(alive.get() > 0);
    drop(output);
    drop(data);
    assert_eq!(alive.get(), 0);
}
struct PollDeserializer(Rc<Cell<u32>>);
impl<'de> serde::Deserializer<'de> for PollDeserializer {
    type Error = serde_json::Error;
    fn deserialize_any<V: serde::de::Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
        self.0.set(
            self.0
                .get()
                .checked_add(1)
                .ok_or_else(|| serde::de::Error::custom("poll count overflow"))?,
        );
        Err(serde::de::Error::custom("polled decoder"))
    }
    serde::forward_to_deserialize_any! { bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier ignored_any }
}

#[test]
fn unit_driver_parks_full_decoder_error_and_preserves_occupied_error_slot() {
    let mut admission = Admission::default();
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let error = serde_json::from_str::<SearchCorpusActiveHeadV1>("{}").unwrap_err();
    let text = error.to_string();
    let mut failure = Some(error);
    let polls = Rc::new(Cell::new(0));
    assert_eq!(
        SearchCorpusActiveHeadV1::try_decode_into_v1(
            PollDeserializer(Rc::clone(&polls)),
            &mut admission,
            &mut data,
            &mut failure
        ),
        Err(quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1::OccupiedOutput)
    );
    assert_eq!(polls.get(), 0);
    assert!(data.is_fresh_v1());
    assert_eq!(failure.as_ref().expect("prior error").to_string(), text);
    failure = None;
    assert_eq!(
        SearchCorpusActiveHeadV1::try_decode_into_v1(
            PollDeserializer(Rc::clone(&polls)),
            &mut admission,
            &mut data,
            &mut failure
        ),
        Err(quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1::OperationRefused)
    );
    assert_eq!(polls.get(), 1);
    assert_eq!(
        failure.as_ref().expect("actual decoder error").to_string(),
        "polled decoder"
    );
    failure = None;
    assert_eq!(
        SearchCorpusActiveHeadV1::try_decode_into_v1(
            PollDeserializer(Rc::clone(&polls)),
            &mut admission,
            &mut data,
            &mut failure
        ),
        Err(quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1::UsedData)
    );
    assert_eq!(polls.get(), 1);
    assert!(failure.is_none());
}
#[test]
fn field_order_and_escaped_wire_preserve_canonical_parity_v1() {
    let head = super::super::qi_act_01_tests::corpus_head(11, "digest-11", 6);
    let wire = serde_json::to_string(&head).expect("fixture");
    let escaped = wire
        .replace("\"repo\"", "\"r\\u0065po\"")
        .replace("\"repo_id\"", "\"repo_\\u0069d\"");
    assert_eq!(
        decode_native(&escaped, &mut Admission::default()).expect("escaped native"),
        head
    );
    assert_eq!(
        serde_json::from_str::<SearchCorpusActiveHeadV1>(&escaped).expect("escaped ordinary"),
        head
    );
    let value = serde_json::to_value(&head).expect("fixture");
    let reversed = format!(
        r#"{{"activation_token":{},"generation":{}}}"#,
        value.get("activation_token").expect("token"),
        value.get("generation").expect("generation")
    );
    assert_eq!(
        decode_native(&reversed, &mut Admission::default()).expect("reordered native"),
        head
    );
    let invalid = wire.replace("digest-11", "");
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut error = None;
    let mut admission = Admission::default();
    let mut decoder = serde_json::Deserializer::from_str(&invalid);
    assert!(
        SearchCorpusActiveHeadV1::try_decode_into_v1(
            &mut decoder,
            &mut admission,
            &mut data,
            &mut error
        )
        .is_err()
    );
    assert!(data.state.output.is_some());
    assert!(
        error
            .as_ref()
            .expect("semantic error")
            .to_string()
            .contains("Semantic")
    );
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_err());
}

#[test]
fn adapter_cannot_report_success_without_the_canonical_producer_v1() {
    let mut admission = Admission {
        skip_producer: true,
        ..Admission::default()
    };
    let mut data = quanta_index_contract_base::NativeIdentityDecodeDataV1::new_v1();
    let mut error = None;
    assert!(
        RepoId::try_decode_into_v1(
            serde::de::value::BorrowedStrDeserializer::<serde_json::Error>::new("repo"),
            &mut admission,
            &mut data,
            &mut error
        )
        .is_err()
    );
    assert!(
        error
            .as_ref()
            .expect("producer refusal")
            .to_string()
            .contains("did not fill")
    );
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_err());
    assert_eq!(admission.copies, 0);
}

#[test]
fn wrong_owned_map_and_track_input_remain_in_external_data_v1() {
    for (wire, depth) in [
        (r#""owned-map-refusal""#, 0),
        (r#"{"generation":"owned-map-refusal"}"#, 1),
        (r#"{"generation":{"lexical":"owned-map-refusal"}}"#, 2),
    ] {
        let mut data = NativeCorpusDecodeDataV1::new_v1();
        let mut admission = Admission::default();
        let mut failure = None;
        let value: serde_json::Value = serde_json::from_str(wire).expect("fixture");
        assert!(
            SearchCorpusActiveHeadV1::try_decode_into_v1(
                value,
                &mut admission,
                &mut data,
                &mut failure
            )
            .is_err()
        );
        let owned = match depth {
            0 => &data.state.keys.refused_map,
            1 => &data.state.generation.keys.refused_map,
            _ => &data.state.generation.lexical.keys.refused_map,
        };
        assert_eq!(owned.as_deref(), Some("owned-map-refusal"));
    }
    let value = serde_json::json!({"generation":{"lexical":{"track":"owned-track-refusal"}}});
    let mut data = NativeCorpusDecodeDataV1::new_v1();
    let mut admission = Admission::default();
    let mut failure = None;
    assert!(
        SearchCorpusActiveHeadV1::try_decode_into_v1(
            value,
            &mut admission,
            &mut data,
            &mut failure
        )
        .is_err()
    );
    assert_eq!(
        data.state.generation.lexical.track.owned.as_deref(),
        Some("owned-track-refusal")
    );
}
