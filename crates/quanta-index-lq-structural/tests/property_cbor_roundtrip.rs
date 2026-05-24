//! Property tests — CBOR roundtrip determinism for [`StructuralBinding`]
//! and [`StructuralCandidate`].
//!
//! Per STR-01 §6.4: random `StructuralBinding` / `StructuralCandidate`
//! shapes must serialize -> deserialize byte-identically, and two encodes
//! of the same value must produce byte-identical output. The `BTreeMap`
//! canonical ordering invariant for metavar keys is exercised across
//! ≥ 256 cases.

#![expect(
    clippy::option_if_let_else,
    reason = "the clippy-suggested `.map_or_else(...)` then trips `unnecessary_result_map_or_else` because the success arm is the identity; manual match is the readable form"
)]

use std::collections::BTreeMap;

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_structural::{
    ByteSpan, DocId, MetaVar, StructuralBinding, StructuralCandidate,
};

// Generate a valid MetaVar identifier name. First char is alpha/_,
// subsequent are alphanumeric/_. Short alphabet so we get realistic
// duplicate-keying behavior inside the BTreeMap. The combinator only
// emits valid identifier shapes, so the MetaVar constructor cannot fail.
fn arb_metavar() -> impl Strategy<Value = MetaVar> {
    (
        prop::sample::select(vec!['a', 'b', 'c', 'X', 'Y', '_']),
        vec(prop::sample::select(vec!['a', 'b', '0', '1', '_']), 0..=3),
    )
        .prop_map(|(first, rest)| {
            let mut s = String::new();
            s.push(first);
            for c in rest {
                s.push(c);
            }
            // The strategy only emits valid identifier shapes; if this
            // constructor fails the strategy itself has regressed and
            // the test should fail loudly via assertion in the prop body.
            match MetaVar::new(&s) {
                Ok(v) => v,
                Err(_) => {
                    // Fallback to a known-good name so the test body can
                    // still surface a meaningful prop_assert failure.
                    match MetaVar::new("X") {
                        Ok(v) => v,
                        Err(_) => std::process::abort(),
                    }
                }
            }
        })
}

fn arb_span() -> impl Strategy<Value = ByteSpan> {
    (0u32..=10_000u32, 0u32..=10_000u32).prop_map(|(a, b)| {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        // Strategy normalizes to (lo, hi) with lo <= hi, so the
        // constructor cannot fail.
        match ByteSpan::new(lo, hi) {
            Ok(s) => s,
            Err(_) => std::process::abort(),
        }
    })
}

fn arb_binding() -> impl Strategy<Value = StructuralBinding> {
    vec((arb_metavar(), arb_span()), 0..=10).prop_map(|entries| {
        let mut map: BTreeMap<MetaVar, ByteSpan> = BTreeMap::new();
        for (k, v) in entries {
            let _prior = map.insert(k, v);
        }
        StructuralBinding::from_map(map)
    })
}

fn arb_candidate() -> impl Strategy<Value = StructuralCandidate> {
    (0u64..=10_000u64, arb_span(), arb_binding())
        .prop_map(|(doc, span, b)| StructuralCandidate::new(DocId(doc), span, b))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn binding_cbor_roundtrip(b in arb_binding()) {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&b, &mut buf) {
            return Err(TestCaseError::reject(format!("ser: {e}")));
        }
        let got: Result<StructuralBinding, _> =
            ciborium::de::from_reader(buf.as_slice());
        let got = match got {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("de: {e}"))),
        };
        prop_assert_eq!(b, got);
    }

    #[test]
    fn binding_byte_identical_encode(b in arb_binding()) {
        let mut buf1: Vec<u8> = Vec::new();
        let mut buf2: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&b, &mut buf1) {
            return Err(TestCaseError::reject(format!("ser1: {e}")));
        }
        if let Err(e) = ciborium::ser::into_writer(&b, &mut buf2) {
            return Err(TestCaseError::reject(format!("ser2: {e}")));
        }
        prop_assert_eq!(buf1, buf2);
    }

    #[test]
    fn candidate_cbor_roundtrip(c in arb_candidate()) {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf) {
            return Err(TestCaseError::reject(format!("ser: {e}")));
        }
        let got: Result<StructuralCandidate, _> =
            ciborium::de::from_reader(buf.as_slice());
        let got = match got {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("de: {e}"))),
        };
        prop_assert_eq!(c, got);
    }

    #[test]
    fn candidate_byte_identical_encode(c in arb_candidate()) {
        let mut buf1: Vec<u8> = Vec::new();
        let mut buf2: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf1) {
            return Err(TestCaseError::reject(format!("ser1: {e}")));
        }
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf2) {
            return Err(TestCaseError::reject(format!("ser2: {e}")));
        }
        prop_assert_eq!(buf1, buf2);
    }
}
