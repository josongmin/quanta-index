use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use ciborium::de::{
    Error as CborDecodeError, FiniteParserAllocationAdmissionV3, FiniteParserAllocationRefusalV3,
    FiniteSemanticReasonV3,
};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    AdmittedByteStringSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1, BatchIngestMode,
    ChunkStructuralMetadata, CorpusDecodeLimitsV1, DecodeAttemptStateV1, DecodeBackingAdmissionV1,
    DecodeBackingRefusalV1, DecodeFieldKindV1, DecodeFieldTokenV1, ManifestGeneration,
    OwnerDocKind, ParserTextInputErrorV1, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SemanticCorpusKindV1,
    SemanticSourceScopeKeyV1, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourceIdentityDecodeAdmissionV1, SourcePublicationEvent, SymbolCoverage,
    SymbolNameSourcePolicyV1,
};
use serde::{Deserialize, Deserializer, de::DeserializeSeed};

use super::{AdmittedCborDecodeErrorV1, AdmittedDecodedCborV1, decode_cbor_seed_admitted_v1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Dispatch/lifetime fixture only. It does not issue a physical memory grant.
#[derive(Clone, Default)]
struct TestOwner {
    attempt: Arc<Mutex<DecodeAttemptStateV1>>,
    aborted: Arc<AtomicBool>,
    reservations: Arc<AtomicUsize>,
    parser_text_rows: Arc<Mutex<Vec<(usize, usize)>>>,
    parser_byte_rows: Arc<Mutex<Vec<(usize, usize)>>>,
    boxed_calls: Arc<AtomicUsize>,
    refuse_boxed_after: Arc<AtomicUsize>,
    deny_text_adoption: Arc<AtomicBool>,
}

impl TestOwner {
    fn with_boxed_limit(limit: usize) -> Self {
        let owner = Self::default();
        owner.refuse_boxed_after.store(limit, Ordering::SeqCst);
        owner
    }

    fn copy_admitted_box(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst)
            || self.boxed_calls.load(Ordering::SeqCst)
                >= self.refuse_boxed_after.load(Ordering::SeqCst)
        {
            return Err(DecodeBackingRefusalV1);
        }
        let boxed = source.into();
        let _previous_calls = self.boxed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(boxed)
    }
}

const STRUCTURAL_CBOR: &[u8] = b"\xa6\x6bvariant_tag\x64node\x73structural_kind_tag\x68function\x77structural_pattern_kind\x6adefinition\x6fstructural_name\x64main\x70structural_scope\x65crate\x77structural_matched_node\x67fn main";

#[test]
fn admitted_structural_metadata_retains_one_owner_for_parser_and_six_boxed_values() -> TestResult {
    let owner = TestOwner::with_boxed_limit(6);
    let witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let mut work_bytes = 0_usize;
    let mut work = |bytes: usize| {
        work_bytes = work_bytes.saturating_add(bytes);
        Ok::<(), ()>(())
    };
    let seed = ChunkStructuralMetadata::deserialize_seed_with_admission_v1(&witness, 32);
    let decoded = decode_cbor_seed_admitted_v1(
        STRUCTURAL_CBOR,
        &mut scratch,
        seed,
        &mut work,
        STRUCTURAL_CBOR.len(),
        owner,
    )
    .map_err(|error| format!("admitted structural CBOR refused: {error:?}"))?;
    let value = decoded.value();
    if value.variant_tag.as_ref() != "node"
        || value.structural_kind_tag.as_ref() != "function"
        || value.structural_pattern_kind.as_ref() != "definition"
        || value.structural_name.as_ref() != "main"
        || value.structural_scope.as_ref() != "crate"
        || value.structural_matched_node.as_ref() != "fn main"
        || witness.reservations.load(Ordering::SeqCst) != 6
        || witness.boxed_calls.load(Ordering::SeqCst) != 6
        || witness.aborted.load(Ordering::SeqCst)
        || work_bytes < STRUCTURAL_CBOR.len().saturating_mul(2)
    {
        return Err("admitted structural CBOR lost a field or shared backing custody".into());
    }
    Ok(())
}

#[test]
fn admitted_structural_failures_abort_parser_and_seed_clones() -> TestResult {
    const MISSING: &[u8] = b"\xa5\x6bvariant_tag\x64node\x73structural_kind_tag\x68function\x77structural_pattern_kind\x6adefinition\x6fstructural_name\x64main\x70structural_scope\x65crate";
    for (wire, boxed_limit, expected_boxed) in [(MISSING, 6, 5), (STRUCTURAL_CBOR, 3, 3)] {
        let owner = TestOwner::with_boxed_limit(boxed_limit);
        let mut witness = owner.clone();
        let mut scratch = [0_u8; 64];
        let mut work = |_bytes| Ok::<(), ()>(());
        let seed = ChunkStructuralMetadata::deserialize_seed_with_admission_v1(&witness, 32);
        let result =
            decode_cbor_seed_admitted_v1(wire, &mut scratch, seed, &mut work, wire.len(), owner);
        if !matches!(result, Err(AdmittedCborDecodeErrorV1::Parser(_)))
            || !witness.aborted.load(Ordering::SeqCst)
            || witness.boxed_calls.load(Ordering::SeqCst) != expected_boxed
            || witness
                .copy_boxed_parser_text(&mut Some("foreign".to_owned()), |_| Ok::<(), ()>(()))
                .is_ok()
            || witness.reserve_text_v3(&mut String::new(), 1).is_ok()
        {
            return Err("structural failure left a detached value or a reusable clone".into());
        }
    }

    let mut duplicate = STRUCTURAL_CBOR.to_vec();
    if let Some(header) = duplicate.first_mut() {
        *header = 0xa7;
    }
    duplicate.extend_from_slice(b"\x6bvariant_tag\x64node");
    let owner = TestOwner::with_boxed_limit(6);
    let witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let mut work = |_bytes| Ok::<(), ()>(());
    let seed = ChunkStructuralMetadata::deserialize_seed_with_admission_v1(&witness, 32);
    let result = decode_cbor_seed_admitted_v1(
        &duplicate,
        &mut scratch,
        seed,
        &mut work,
        duplicate.len(),
        owner,
    );
    if !matches!(result, Err(AdmittedCborDecodeErrorV1::Parser(_)))
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.boxed_calls.load(Ordering::SeqCst) != 6
    {
        return Err("duplicate structural key reached a seventh value or survived abort".into());
    }
    Ok(())
}

#[test]
fn field_text_header_refuses_before_parser_backing_birth() -> TestResult {
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 4];
    let mut work = |_bytes| Ok::<(), ()>(());
    let result = decode_cbor_seed_admitted_v1(
        b"\x65abcde",
        &mut scratch,
        AdmittedTextSeedV1::new(&witness, 4),
        &mut work,
        6,
        owner,
    );
    if !matches!(
        result,
        Err(AdmittedCborDecodeErrorV1::Parser(
            CborDecodeError::Semantic(
                _,
                FiniteSemanticReasonV3::Static("controlled CBOR text header refused")
            )
        ))
    ) || witness.reservations.load(Ordering::SeqCst) != 0
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err(
            "five-byte definite text reached parser backing under four-byte field cap".into(),
        );
    }
    Ok(())
}

#[test]
fn field_byte_string_and_byte_array_headers_refuse_before_backing_birth() -> TestResult {
    for (wire, expected_reason) in [
        (
            b"\x43\x01\x02\x03".as_slice(),
            "controlled CBOR bytes header refused",
        ),
        (
            b"\x83\x01\x02\x03".as_slice(),
            "controlled CBOR array header refused",
        ),
    ] {
        let owner = TestOwner::default();
        let witness = owner.clone();
        let mut scratch = [0_u8; 4];
        let mut work = |_bytes| Ok::<(), ()>(());
        let result = decode_cbor_seed_admitted_v1(
            wire,
            &mut scratch,
            AdmittedByteStringSeedV1::new(&witness, 2),
            &mut work,
            wire.len(),
            owner,
        );
        if !matches!(
            result,
            Err(AdmittedCborDecodeErrorV1::Parser(CborDecodeError::Semantic(
                _,
                FiniteSemanticReasonV3::Static(reason)
            ))) if reason == expected_reason
        ) || witness.reservations.load(Ordering::SeqCst) != 0
            || !witness.aborted.load(Ordering::SeqCst)
        {
            return Err("oversized byte header entered parser or typed Vec allocation".into());
        }
    }
    Ok(())
}

#[test]
fn chunked_text_growth_keeps_prefix_and_refuses_the_first_over_limit_append() -> TestResult {
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 4];
    let mut work = |_bytes| Ok::<(), ()>(());
    let accepted = decode_cbor_seed_admitted_v1(
        b"\x7f\x62ab\x62cd\xff",
        &mut scratch,
        AdmittedTextSeedV1::new(&witness, 4),
        &mut work,
        8,
        owner,
    )
    .map_err(|error| format!("bounded chunked text was refused: {error:?}"))?;
    if accepted.value() != "abcd" || witness.reservations.load(Ordering::SeqCst) != 2 {
        return Err("accepted chunked text lost a segment or allocation dispatch".into());
    }

    let owner = TestOwner::default();
    let witness = owner.clone();
    let result = decode_cbor_seed_admitted_v1(
        b"\x7f\x62ab\x62cd\x62ef\xff",
        &mut scratch,
        AdmittedTextSeedV1::new(&witness, 4),
        &mut work,
        11,
        owner,
    );
    if !matches!(
        result,
        Err(AdmittedCborDecodeErrorV1::Parser(
            CborDecodeError::Semantic(
                _,
                FiniteSemanticReasonV3::Static("controlled CBOR text append refused")
            )
        ))
    ) || witness.reservations.load(Ordering::SeqCst) != 2
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("third chunk grew the parser backing past the field cap".into());
    }
    Ok(())
}

#[test]
fn parser_reentry_and_unproven_owned_input_poison_the_shared_attempt() -> TestResult {
    let owner = TestOwner::default();
    let mut parser = owner.clone();
    parser
        .begin_decode_v3()
        .map_err(|_refusal| "fixture parser begin refused")?;
    if parser.begin_decode_v3().is_ok()
        || owner
            .enter_decode_field_v1(DecodeFieldKindV1::Text, 8)
            .is_ok()
    {
        return Err("parser clone accepted a second logical attempt".into());
    }

    let owner = TestOwner::default();
    let mut parser = owner.clone();
    parser
        .begin_decode_v3()
        .map_err(|_refusal| "fixture parser begin refused")?;
    let mut input = Some(String::from("foreign"));
    if owner
        .adopt_parser_text(&mut input, |_| Ok::<(), ()>(()))
        .is_ok()
        || input.as_deref() != Some("foreign")
        || owner
            .enter_decode_field_v1(DecodeFieldKindV1::Text, 8)
            .is_ok()
    {
        return Err("unproven parser input was consumed or the attempt stayed reusable".into());
    }
    Ok(())
}

#[test]
fn nested_sequences_accept_independent_inner_count_and_refuse_outer_header() -> TestResult {
    // [[1, 2, 3]]: the outer Vec has one child; that child's Vec has three.
    let wire = b"\x81\x83\x01\x02\x03";
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 4];
    let mut work = |_bytes| Ok::<(), ()>(());
    let seed = AdmittedSequenceSeedV1::new(&witness, 1, || {
        AdmittedSequenceSeedV1::new(&witness, 3, || TypedSeed::<u8>(PhantomData))
    });
    let decoded =
        decode_cbor_seed_admitted_v1(wire, &mut scratch, seed, &mut work, wire.len(), owner)
            .map_err(|error| format!("independent nested sequence bounds refused: {error:?}"))?;
    if decoded.value() != &vec![vec![1, 2, 3]]
        || witness.reservations.load(Ordering::SeqCst) != 4
        || witness.aborted.load(Ordering::SeqCst)
    {
        return Err("inner three-item Vec failed beneath one-item outer Vec".into());
    }

    // [[1, 2, 3], [4, 5, 6]]: the two-child outer header is over its cap.
    let wire = b"\x82\x83\x01\x02\x03\x83\x04\x05\x06";
    let owner = TestOwner::default();
    let witness = owner.clone();
    let seed = AdmittedSequenceSeedV1::new(&witness, 1, || {
        AdmittedSequenceSeedV1::new(&witness, 3, || TypedSeed::<u8>(PhantomData))
    });
    let refused =
        decode_cbor_seed_admitted_v1(wire, &mut scratch, seed, &mut work, wire.len(), owner);
    if !matches!(
        refused,
        Err(AdmittedCborDecodeErrorV1::Parser(
            CborDecodeError::Semantic(
                _,
                FiniteSemanticReasonV3::Static("controlled CBOR array header refused")
            )
        ))
    ) || witness.reservations.load(Ordering::SeqCst) != 0
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("outer over-cap array reached a child or backing allocation".into());
    }
    Ok(())
}

#[test]
fn outer_sequence_cap_does_not_apply_to_implicit_nested_fixed_array() -> TestResult {
    // The outer Vec owns one item; [u8; 3] has no separately admitted field.
    let wire = b"\x81\x83\x01\x02\x03";
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 4];
    let mut work = |_bytes| Ok::<(), ()>(());
    let seed = AdmittedSequenceSeedV1::new(&witness, 1, || TypedSeed::<[u8; 3]>(PhantomData));
    let decoded =
        decode_cbor_seed_admitted_v1(wire, &mut scratch, seed, &mut work, wire.len(), owner)
            .map_err(|error| format!("nested fixed array refused: {error:?}"))?;
    if decoded.value() != &vec![[1, 2, 3]]
        || witness.reservations.load(Ordering::SeqCst) != 1
        || witness.aborted.load(Ordering::SeqCst)
    {
        return Err("nested fixed array inherited outer item cap or backing changed".into());
    }

    // The two-item outer header is refused before any child is born.
    let wire = b"\x82\x83\x01\x02\x03\x83\x04\x05\x06";
    let owner = TestOwner::default();
    let witness = owner.clone();
    let seed = AdmittedSequenceSeedV1::new(&witness, 1, || TypedSeed::<[u8; 3]>(PhantomData));
    let refused =
        decode_cbor_seed_admitted_v1(wire, &mut scratch, seed, &mut work, wire.len(), owner);
    if !matches!(
        refused,
        Err(AdmittedCborDecodeErrorV1::Parser(
            CborDecodeError::Semantic(
                _,
                FiniteSemanticReasonV3::Static("controlled CBOR array header refused")
            )
        ))
    ) || witness.reservations.load(Ordering::SeqCst) != 0
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("over-cap outer header reached nested fixed array".into());
    }
    Ok(())
}

impl FiniteParserAllocationAdmissionV3 for TestOwner {
    fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .begin_preflight_v1()
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .complete_preflight_v1()
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .begin_decode_v1()
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .complete_decode_v1()
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn abort_decode_v3(&mut self) {
        self.aborted.store(true, Ordering::SeqCst);
        if let Ok(mut attempt) = self.attempt.lock() {
            attempt.abort_v1();
        }
    }

    fn check_text_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .check_length_v1(DecodeFieldKindV1::Text, length, 4096)
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn check_bytes_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .check_length_v1(DecodeFieldKindV1::Bytes, length, 4096)
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn check_sequence_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.attempt
            .lock()
            .map_err(|_poison| FiniteParserAllocationRefusalV3)?
            .check_sequence_header_v1(length, 4096)
            .map_err(|_attempt_error| FiniteParserAllocationRefusalV3)
    }

    fn reserve_text_v3(
        &mut self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        let length = destination
            .len()
            .checked_add(additional)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        self.check_text_length_v3(Some(length))?;
        if self.aborted.load(Ordering::SeqCst) || destination.try_reserve_exact(additional).is_err()
        {
            return Err(FiniteParserAllocationRefusalV3);
        }
        let Ok(mut rows) = self.parser_text_rows.lock() else {
            return Err(FiniteParserAllocationRefusalV3);
        };
        rows.push((destination.as_ptr().addr(), destination.capacity()));
        let _previous_reservations = self.reservations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn reserve_bytes_v3(
        &mut self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        let length = destination
            .len()
            .checked_add(additional)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        self.check_bytes_length_v3(Some(length))?;
        if self.aborted.load(Ordering::SeqCst) || destination.try_reserve_exact(additional).is_err()
        {
            return Err(FiniteParserAllocationRefusalV3);
        }
        let Ok(mut rows) = self.parser_byte_rows.lock() else {
            return Err(FiniteParserAllocationRefusalV3);
        };
        rows.push((destination.as_ptr().addr(), destination.capacity()));
        let _previous_reservations = self.reservations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl DecodeBackingAdmissionV1 for TestOwner {
    fn abort_decode(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        if let Ok(mut attempt) = self.attempt.lock() {
            attempt.abort_v1();
        }
    }

    fn enter_decode_field_v1(
        &self,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        self.attempt
            .lock()
            .map_err(|_poison| DecodeBackingRefusalV1)?
            .enter_field_v1(kind, maximum)
    }

    fn leave_decode_field_v1(
        &self,
        token: DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.attempt
            .lock()
            .map_err(|_poison| DecodeBackingRefusalV1)?
            .leave_field_v1(token)
    }

    fn reserve_text(
        &self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst)
            || self.deny_text_adoption.load(Ordering::SeqCst)
            || destination.try_reserve_exact(additional).is_err()
        {
            return Err(DecodeBackingRefusalV1);
        }
        let _previous_reservations = self.reservations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst) || destination.try_reserve_exact(additional).is_err()
        {
            return Err(DecodeBackingRefusalV1);
        }
        let _previous_reservations = self.reservations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst) || destination.try_reserve_exact(additional).is_err()
        {
            return Err(DecodeBackingRefusalV1);
        }
        let _previous_reservations = self.reservations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        self.copy_admitted_box(source)
    }

    fn adopt_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, ParserTextInputErrorV1<E>> {
        self.attempt
            .lock()
            .map_err(|_poison| DecodeBackingRefusalV1)?
            .begin_owned_input_v1()?;
        let result = (|| {
            let value = source.as_ref().ok_or(DecodeBackingRefusalV1)?;
            let rows = self
                .parser_text_rows
                .lock()
                .map_err(|_poison| DecodeBackingRefusalV1)?;
            if self.aborted.load(Ordering::SeqCst)
                || self.deny_text_adoption.load(Ordering::SeqCst)
                || !rows.contains(&(value.as_ptr().addr(), value.capacity()))
            {
                return Err(ParserTextInputErrorV1::Admission(DecodeBackingRefusalV1));
            }
            drop(rows);
            validate(value).map_err(ParserTextInputErrorV1::Validation)?;
            source.take().ok_or_else(|| DecodeBackingRefusalV1.into())
        })();
        if let Ok(mut attempt) = self.attempt.lock() {
            attempt.finish_owned_input_v1(result.is_ok());
        }
        result
    }

    fn copy_boxed_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, ParserTextInputErrorV1<E>> {
        self.attempt
            .lock()
            .map_err(|_poison| DecodeBackingRefusalV1)?
            .begin_owned_input_v1()?;
        let result = (|| {
            let value = source.as_ref().ok_or(DecodeBackingRefusalV1)?;
            let rows = self
                .parser_text_rows
                .lock()
                .map_err(|_poison| DecodeBackingRefusalV1)?;
            if self.aborted.load(Ordering::SeqCst)
                || !rows.contains(&(value.as_ptr().addr(), value.capacity()))
            {
                return Err(ParserTextInputErrorV1::Admission(DecodeBackingRefusalV1));
            }
            drop(rows);
            validate(value).map_err(ParserTextInputErrorV1::Validation)?;
            let boxed = self.copy_admitted_box(value)?;
            let _consumed = source.take().ok_or(DecodeBackingRefusalV1)?;
            Ok(boxed)
        })();
        if let Ok(mut attempt) = self.attempt.lock() {
            attempt.finish_owned_input_v1(result.is_ok());
        }
        result
    }

    fn verify_parser_owned_bytes(
        &self,
        source: &[u8],
        capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        let Ok(rows) = self.parser_byte_rows.lock() else {
            return Err(DecodeBackingRefusalV1);
        };
        if self.aborted.load(Ordering::SeqCst)
            || !rows.contains(&(source.as_ptr().addr(), capacity))
        {
            return Err(DecodeBackingRefusalV1);
        }
        Ok(())
    }
}

// The fixture checks shared parser/seed dispatch only. Production identity
// decoding must use Original-funded native DATA, not these ordinary constructors.
impl SourceIdentityDecodeAdmissionV1 for TestOwner {
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

struct TypedSeed<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for TypedSeed<T> {
    type Value = T;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<T, D::Error> {
        T::deserialize(decoder)
    }
}

struct RejectAfterString;

impl<'de> DeserializeSeed<'de> for RejectAfterString {
    type Value = String;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
        let _parsed = String::deserialize(decoder)?;
        Err(serde::de::Error::custom(
            "typed seed rejected completed text",
        ))
    }
}

#[test]
fn fixed_cbor_string_keeps_its_parser_owner_and_charges_both_passes() -> TestResult {
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let work_bytes = Cell::new(0_usize);
    let mut work = |bytes: usize| {
        work_bytes.set(work_bytes.get().saturating_add(bytes));
        Ok::<(), ()>(())
    };
    let decoded = decode_cbor_seed_admitted_v1(
        b"\x65hello",
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut work,
        6,
        owner,
    )
    .map_err(|error| format!("fixed CBOR unexpectedly refused: {error:?}"))?;
    if decoded.value() != "hello"
        || decoded.owner().aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) == 0
        || work_bytes.get() < 12
    {
        return Err("fixed CBOR lost the value, owner or two-pass work".into());
    }
    Ok(())
}

#[test]
fn frame_and_input_refusals_abort_before_parser_reservations() -> TestResult {
    let mut nested = vec![0x81; 257];
    nested.push(0x01);
    for (input, maximum) in [
        (b"\x01\x02".as_slice(), 2),
        (b"\x82\x01".as_slice(), 2),
        (nested.as_slice(), nested.len()),
        (b"\x01".as_slice(), 0),
    ] {
        let owner = TestOwner::default();
        let witness = owner.clone();
        let mut scratch = [0_u8; 64];
        let mut work = |_bytes| Ok::<(), ()>(());
        let result = decode_cbor_seed_admitted_v1(
            input,
            &mut scratch,
            TypedSeed::<u64>(PhantomData),
            &mut work,
            maximum,
            owner,
        );
        if !matches!(
            result,
            Err(AdmittedCborDecodeErrorV1::Frame(_)
                | AdmittedCborDecodeErrorV1::InputTooLarge { .. })
        ) || !witness.aborted.load(Ordering::SeqCst)
            || witness.reservations.load(Ordering::SeqCst) != 0
        {
            return Err("invalid CBOR reached parser reservation or escaped typed refusal".into());
        }
    }
    Ok(())
}

#[test]
fn first_preflight_work_refusal_survives_io_error_without_retry() -> TestResult {
    #[derive(Debug, Eq, PartialEq)]
    enum WorkCause {
        First,
        Later,
    }

    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let calls = Cell::new(0_u8);
    let mut work = |bytes: usize| {
        if bytes == 0 {
            return Ok(());
        }
        calls.set(calls.get().saturating_add(1));
        Err(if calls.get() == 1 {
            WorkCause::First
        } else {
            WorkCause::Later
        })
    };
    let result = decode_cbor_seed_admitted_v1(
        b"\x65hello",
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut work,
        6,
        owner,
    );
    if !matches!(
        result,
        Err(AdmittedCborDecodeErrorV1::Work(WorkCause::First))
    ) || calls.get() != 1
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) != 0
    {
        return Err("preflight overwrote its first work refusal or retried after it".into());
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the fixed panic is the catch_unwind witness for preflight owner abort"
)]
fn first_preflight_work_panic_aborts_shared_owner_before_parser_birth() -> TestResult {
    let owner = TestOwner::default();
    let mut witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut work =
            |_bytes: usize| -> Result<(), ()> { panic!("fixed first preflight work panic") };
        drop(decode_cbor_seed_admitted_v1(
            b"\x65hello",
            &mut scratch,
            TypedSeed::<String>(PhantomData),
            &mut work,
            6,
            owner,
        ));
    }));
    if result.is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) != 0
        || witness.begin_decode_v3().is_ok()
        || witness.reserve_text_v3(&mut String::new(), 1).is_ok()
    {
        return Err("preflight work panic left a reusable parser owner".into());
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the nested assertion checks refusal during the reentrant work callback"
)]
fn preflight_work_reentry_refuses_before_nested_work_or_parser_birth() -> TestResult {
    let owner = TestOwner::default();
    let witness = owner.clone();
    let reentered = Cell::new(false);
    let nested_work_calls = Cell::new(0_usize);
    let mut scratch = [0_u8; 4];
    let mut work = |_bytes: usize| {
        if !reentered.replace(true) {
            let mut nested_scratch = [0_u8; 4];
            let mut nested_work = |_bytes: usize| {
                nested_work_calls.set(nested_work_calls.get() + 1);
                Ok::<(), ()>(())
            };
            let nested = decode_cbor_seed_admitted_v1(
                b"\x65hello",
                &mut nested_scratch,
                TypedSeed::<String>(PhantomData),
                &mut nested_work,
                6,
                witness.clone(),
            );
            assert!(matches!(
                nested,
                Err(AdmittedCborDecodeErrorV1::Parser(
                    CborDecodeError::Semantic(
                        _,
                        FiniteSemanticReasonV3::Static("controlled CBOR preflight attempt refused")
                    )
                ))
            ));
        }
        Ok::<(), ()>(())
    };
    let result = decode_cbor_seed_admitted_v1(
        b"\x65hello",
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut work,
        6,
        owner,
    );
    let mut after = witness.clone();
    if !reentered.get()
        || nested_work_calls.get() != 0
        || !matches!(
            result,
            Err(AdmittedCborDecodeErrorV1::Parser(
                CborDecodeError::Semantic(
                    _,
                    FiniteSemanticReasonV3::Static("controlled CBOR preflight completion refused")
                )
            ))
        )
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) != 0
        || after.begin_preflight_v3().is_ok()
    {
        return Err("preflight reentry reached work or failed to poison outer attempt".into());
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the fixed assertion panic is the catch_unwind witness before parser reservation"
)]
fn preflight_read_work_panic_aborts_before_parser_reservation() -> TestResult {
    let owner = TestOwner::default();
    let mut witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut work = |bytes: usize| -> Result<(), ()> {
            assert_eq!(bytes, 0, "fixed preflight read work panic");
            Ok(())
        };
        drop(decode_cbor_seed_admitted_v1(
            b"\x65hello",
            &mut scratch,
            TypedSeed::<String>(PhantomData),
            &mut work,
            6,
            owner,
        ));
    }));
    if result.is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) != 0
        || witness.begin_decode_v3().is_ok()
    {
        return Err("preflight read panic left a reusable parser owner".into());
    }
    Ok(())
}

#[test]
fn parser_read_work_refusal_keeps_its_original_cause() -> TestResult {
    #[derive(Debug, Eq, PartialEq)]
    enum WorkCause {
        ParserRead,
    }

    let input = b"\x65hello";
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let preflight_bytes = Cell::new(0_usize);
    let mut work = |bytes: usize| {
        if bytes > 0 && preflight_bytes.get() == input.len() {
            return Err(WorkCause::ParserRead);
        }
        preflight_bytes.set(preflight_bytes.get().saturating_add(bytes));
        Ok(())
    };
    let result = decode_cbor_seed_admitted_v1(
        input,
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut work,
        input.len(),
        owner,
    );
    if !matches!(
        result,
        Err(AdmittedCborDecodeErrorV1::Work(WorkCause::ParserRead))
    ) || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) != 0
    {
        return Err("parser read lost the original work refusal".into());
    }
    Ok(())
}

#[test]
fn short_scratch_refuses_before_work_or_parser_reservation() -> TestResult {
    let owner = TestOwner::default();
    let witness = owner.clone();
    let mut scratch = [0_u8; 3];
    let work_calls = Cell::new(0_u8);
    let mut work = |_bytes| {
        work_calls.set(work_calls.get().saturating_add(1));
        Ok::<(), ()>(())
    };
    let result = decode_cbor_seed_admitted_v1(
        b"\x01",
        &mut scratch,
        TypedSeed::<u64>(PhantomData),
        &mut work,
        1,
        owner,
    );
    if !matches!(
        result,
        Err(AdmittedCborDecodeErrorV1::ScratchTooSmall { actual: 3 })
    ) || work_calls.get() != 0
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) != 0
    {
        return Err("short scratch reached work or parser admission".into());
    }
    Ok(())
}

#[test]
fn partial_parser_allocation_then_typed_failure_poisoned_every_owner_clone() -> TestResult {
    let owner = TestOwner::default();
    let mut witness = owner.clone();
    let mut scratch = [0_u8; 64];
    let mut work = |_bytes| Ok::<(), ()>(());
    let result = decode_cbor_seed_admitted_v1(
        b"\x65hello",
        &mut scratch,
        RejectAfterString,
        &mut work,
        6,
        owner,
    );
    if !matches!(result, Err(AdmittedCborDecodeErrorV1::Parser(_)))
        || !witness.aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) == 0
        || witness.reserve_text_v3(&mut String::new(), 1).is_ok()
    {
        return Err("partial parser failure left a live clone reusable".into());
    }
    Ok(())
}

#[test]
fn carrier_drops_value_before_its_owner() -> TestResult {
    struct Value(Rc<Cell<u8>>);
    impl Drop for Value {
        fn drop(&mut self) {
            self.0.set(1);
        }
    }
    struct Owner(Rc<Cell<u8>>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.set(if self.0.get() == 1 { 2 } else { 3 });
        }
    }

    let order = Rc::new(Cell::new(0));
    let carrier = AdmittedDecodedCborV1 {
        value: Value(Rc::clone(&order)),
        owner: Owner(Rc::clone(&order)),
    };
    drop(carrier);
    if order.get() != 2 {
        return Err("decoded value was dropped after its owner".into());
    }
    Ok(())
}

fn corpus_limits() -> CorpusDecodeLimitsV1 {
    CorpusDecodeLimitsV1 {
        text_bytes: 64,
        source_bytes: 2,
        bundle_bytes: 2,
        collection_items: 1,
    }
}

fn corpus_fixture() -> TestResultValue<SearchCorpusIngestBatch> {
    let repo = RepoId::new("r")?;
    let revision = RevisionId::new("v")?;
    let language = LanguageCode::from_code_str("rust").ok_or("missing fixture language")?;
    Ok(SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "s".into(),
            event_id: "e".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "m".into(),
        batch_digest: "b".into(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: Some(vec![1, 2]),
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            coverage: SourceFileCoverage {
                source: SourceFileRevision {
                    file: SourceFileKey {
                        source_repo_id: repo,
                        repo_relative_path: RepoRelativePath::new("a.rs"),
                    },
                    revision_id: revision,
                    source_sha256: [0; 32],
                },
                language,
                producer_policy_sha256: [0; 32],
                symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: [0; 32],
                text_admitted: true,
                symbols: SymbolCoverage::NotRequested,
            },
            source_bytes: vec![1, 2],
            chunks: Vec::new(),
            symbols: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

type TestResultValue<T> = Result<T, Box<dyn std::error::Error>>;

fn decode_corpus(
    wire: &[u8],
    scratch: &mut [u8],
    owner: TestOwner,
    limits: CorpusDecodeLimitsV1,
) -> Result<AdmittedDecodedCborV1<SearchCorpusIngestBatch, TestOwner>, String> {
    let witness = owner.clone();
    let mut work = |_bytes| Ok::<(), ()>(());
    decode_cbor_seed_admitted_v1(
        wire,
        scratch,
        SearchCorpusIngestBatch::deserialize_seed_with_admission_v1(&witness, limits),
        &mut work,
        wire.len(),
        owner,
    )
    .map_err(|error| format!("controlled corpus CBOR refused: {error:?}"))
}

#[test]
fn corpus_writer_byte_string_and_bundle_array_use_one_controlled_owner() -> TestResult {
    let expected = corpus_fixture()?;
    let mut wire = Vec::new();
    ciborium::into_writer(&expected, &mut wire)?;
    if !wire
        .windows(b"\x6csource_bytes\x42\x01\x02".len())
        .any(|window| window == b"\x6csource_bytes\x42\x01\x02")
        || !wire
            .windows(b"\x6ebundle_payload\x82\x01\x02".len())
            .any(|window| window == b"\x6ebundle_payload\x82\x01\x02")
    {
        return Err("canonical source bytes or bundle representation changed".into());
    }
    let owner = TestOwner::with_boxed_limit(8);
    let witness = owner.clone();
    let mut scratch = [0_u8; 25];
    let decoded = decode_corpus(&wire, &mut scratch, owner, corpus_limits())?;
    if decoded.value() != &expected
        || decoded.owner().aborted.load(Ordering::SeqCst)
        || witness.reservations.load(Ordering::SeqCst) == 0
        || witness
            .parser_byte_rows
            .lock()
            .map_err(|_poisoned_fixture| "parser byte fixture lock poisoned")?
            .is_empty()
    {
        return Err("corpus CBOR did not preserve nested bytes and retained owner".into());
    }

    // The same existing Vec<u8> reader accepts a CBOR byte string for the
    // bundle. Change only its wire major type, keeping both payload bytes.
    let marker = b"\x6ebundle_payload\x82\x01\x02";
    let offset = wire
        .windows(marker.len())
        .position(|window| window == marker)
        .ok_or("missing fixed bundle marker")?;
    let header = offset
        .checked_add(marker.len() - 3)
        .ok_or("bundle offset overflow")?;
    *wire.get_mut(header).ok_or("missing bundle value header")? = 0x42;
    let owner = TestOwner::with_boxed_limit(8);
    let mut scratch = [0_u8; 25];
    let decoded = decode_corpus(&wire, &mut scratch, owner, corpus_limits())?;
    if decoded.value().bundle_payload.as_deref() != Some(&[1, 2][..])
        || decoded.value().replace_scopes != expected.replace_scopes
    {
        return Err("admitted Ciborium byte-string bundle changed its value".into());
    }

    let mut no_source = corpus_limits();
    no_source.source_bytes = 1;
    let owner = TestOwner::with_boxed_limit(8);
    let witness = owner.clone();
    if decode_corpus(&wire, &mut scratch, owner, no_source).is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("oversized CBOR source byte string entered the admitted DTO".into());
    }
    let mut no_bundle = corpus_limits();
    no_bundle.bundle_bytes = 1;
    let owner = TestOwner::with_boxed_limit(8);
    let witness = owner.clone();
    if decode_corpus(&wire, &mut scratch, owner, no_bundle).is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("oversized CBOR bundle byte string entered the admitted DTO".into());
    }
    Ok(())
}

#[test]
fn corpus_longest_known_key_requires_borrowed_identifier_scratch() -> TestResult {
    let expected = corpus_fixture()?;
    let mut wire = Vec::new();
    ciborium::into_writer(&expected, &mut wire)?;
    let owner = TestOwner::with_boxed_limit(8);
    let witness = owner.clone();
    let mut scratch = [0_u8; 24];
    if decode_corpus(&wire, &mut scratch, owner, corpus_limits()).is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("24-byte scratch accepted the 25-byte known corpus key".into());
    }
    let mut scratch = [0_u8; 25];
    let decoded = decode_corpus(
        &wire,
        &mut scratch,
        TestOwner::with_boxed_limit(8),
        corpus_limits(),
    )?;
    if decoded.value() != &expected {
        return Err("25-byte scratch rejected canonical corpus writer wire".into());
    }
    Ok(())
}

#[test]
fn nested_semantic_scope_text_and_collection_limits_abort_actual_cbor() -> TestResult {
    let mut source = corpus_fixture()?;
    source.replace_scopes.clear();
    source.bundle_payload = None;
    source
        .semantic_tombstone_scopes
        .push(SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            owner_kind: OwnerDocKind::File,
            owner_id: "ab".into(),
        });
    let mut wire = Vec::new();
    ciborium::into_writer(&source, &mut wire)?;
    let mut scratch = [0_u8; 25];
    let admitted = decode_corpus(
        &wire,
        &mut scratch,
        TestOwner::with_boxed_limit(8),
        corpus_limits(),
    )?;
    if admitted.value() != &source {
        return Err("nested semantic scope lost canonical writer fields".into());
    }
    let mut no_text = corpus_limits();
    no_text.text_bytes = 1;
    let owner = TestOwner::with_boxed_limit(8);
    let witness = owner.clone();
    if decode_corpus(&wire, &mut scratch, owner, no_text).is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("oversized nested semantic owner text survived admission".into());
    }
    let mut no_collection = corpus_limits();
    no_collection.collection_items = 0;
    let owner = TestOwner::with_boxed_limit(8);
    let witness = owner.clone();
    if decode_corpus(&wire, &mut scratch, owner, no_collection).is_ok()
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("nested semantic collection exceeded its zero-item bound".into());
    }
    Ok(())
}

#[test]
fn admitted_history_response_matches_canonical_receipts_and_keeps_text_owner() -> TestResult {
    use quanta_index_contract::{
        BatchPublishReceipt, SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
        SemanticContentRootsV1, admitted_auxiliary_response_seed_v1,
    };

    let receipt = BatchPublishReceipt::empty_for(
        ManifestGeneration::new(9),
        Some("sha256:fixture".to_owned()),
        "a".repeat(64),
    )
    .recorded_at(17);
    let mut with_roots = receipt.clone();
    with_roots.semantic_content = Some(SemanticContentRootsV1 {
        row_root_digest: format!("sha256:{}", "a".repeat(64)),
        membership_root_digest: format!("sha256:{}", "b".repeat(64)),
    });
    let variants = [
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt.clone()),
        SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt.clone()),
        SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt.clone()),
        SearchPlaneIngestIpcResponse::DirtyReceipt(receipt.clone()),
        SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
        SearchPlaneIngestIpcResponse::HistoryReceipt(with_roots),
    ];
    for payload in variants {
        let expected = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 71,
            payload,
        };
        let wire = crate::encode_cbor_payload(&expected)?;
        let ordinary: SearchPlaneIngestIpcResponseEnvelope = crate::decode_cbor_payload(&wire)?;
        let owner = TestOwner::default();
        let witness = owner.clone();
        let seed_owner = owner.clone();
        let mut scratch = [0_u8; 128];
        let mut charged_work = 0_usize;
        let decoded = decode_cbor_seed_admitted_v1(
            &wire,
            &mut scratch,
            admitted_auxiliary_response_seed_v1(&seed_owner, wire.len()),
            &mut |bytes| {
                charged_work = charged_work.saturating_add(bytes);
                Ok::<(), ()>(())
            },
            wire.len(),
            owner,
        )
        .map_err(|error| format!("admitted History response refused: {error:?}"))?;
        if decoded.value() != &ordinary
            || witness.reservations.load(Ordering::SeqCst) == 0
            || witness.aborted.load(Ordering::SeqCst)
            || charged_work < wire.len().saturating_mul(2)
        {
            return Err("admitted History receipt differs from canonical wire or owner".into());
        }
        drop(decoded);
    }
    Ok(())
}

#[test]
fn admitted_history_response_refuses_text_adoption_before_returning_receipt() -> TestResult {
    use quanta_index_contract::{
        BatchPublishReceipt, SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
        admitted_auxiliary_response_seed_v1,
    };

    let expected = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 17,
        payload: SearchPlaneIngestIpcResponse::HistoryReceipt(BatchPublishReceipt::empty_for(
            ManifestGeneration::new(3),
            None,
            "b".repeat(64),
        )),
    };
    let wire = crate::encode_cbor_payload(&expected)?;
    let owner = TestOwner::default();
    let witness = owner.clone();
    let seed_owner = owner.clone();
    owner.deny_text_adoption.store(true, Ordering::SeqCst);
    let mut scratch = [0_u8; 128];
    let decoded = decode_cbor_seed_admitted_v1(
        &wire,
        &mut scratch,
        admitted_auxiliary_response_seed_v1(&seed_owner, wire.len()),
        &mut |_bytes| Ok::<(), ()>(()),
        wire.len(),
        owner,
    );
    if !matches!(decoded, Err(AdmittedCborDecodeErrorV1::Parser(_)))
        || !witness.aborted.load(Ordering::SeqCst)
    {
        return Err("denied History text backing returned a receipt".into());
    }
    Ok(())
}

#[test]
fn admitted_history_response_preserves_remote_error_and_refuses_foreign_route() -> TestResult {
    use quanta_index_contract::{
        BatchPublishReceipt, QueryErrorRepair, RepairClass, SearchPlaneErrorCodeV2,
        SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError,
        admitted_auxiliary_response_seed_v1,
    };

    let responses = [
        (
            SearchPlaneIngestIpcResponse::Error(SearchPlaneIpcError {
                code: SearchPlaneErrorCodeV2::InvalidRequest,
                message: "bad History body".to_owned(),
                repair: None,
            }),
            true,
        ),
        (
            SearchPlaneIngestIpcResponse::Error(SearchPlaneIpcError {
                code: SearchPlaneErrorCodeV2::InvalidRequest,
                message: "repair-carrying wire".to_owned(),
                repair: Some(QueryErrorRepair {
                    class: RepairClass::Unsupported,
                    supported_alternatives: vec!["repo:".to_owned()],
                    docs_anchor: Some("docs/query".to_owned()),
                }),
            }),
            true,
        ),
        (
            SearchPlaneIngestIpcResponse::RepoTopicReceipt(BatchPublishReceipt::empty_for(
                ManifestGeneration::new(4),
                None,
                "c".repeat(64),
            )),
            false,
        ),
    ];
    for (payload, accepted) in responses {
        let expected = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 81,
            payload,
        };
        let wire = crate::encode_cbor_payload(&expected)?;
        let owner = TestOwner::default();
        let witness = owner.clone();
        let seed_owner = owner.clone();
        let mut scratch = [0_u8; 128];
        let decoded = decode_cbor_seed_admitted_v1(
            &wire,
            &mut scratch,
            admitted_auxiliary_response_seed_v1(&seed_owner, wire.len()),
            &mut |_bytes| Ok::<(), ()>(()),
            wire.len(),
            owner,
        );
        if accepted {
            let decoded = decoded.map_err(|error| format!("admitted error refused: {error:?}"))?;
            if decoded.value() != &expected || witness.aborted.load(Ordering::SeqCst) {
                return Err("admitted remote error changed canonical response".into());
            }
        } else if !matches!(decoded, Err(AdmittedCborDecodeErrorV1::Parser(_)))
            || !witness.aborted.load(Ordering::SeqCst)
        {
            return Err("foreign ingest route crossed History admitted decoder".into());
        }
    }
    Ok(())
}
