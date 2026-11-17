//! Normal-engine facade of the canonical compact Native root.
#[cfg(test)]
use crate::legacy::native_root::NATIVE_FORMAT;
pub(crate) use crate::legacy::native_root::NativeRoot;
use crate::lexical_runs::RunRegistry;
use crate::lexical_store::MapRoot;
impl NativeRoot {
    pub(crate) fn empty(allocation_scope_sha256: [u8; 32]) -> Self {
        Self {
            allocation_scope_sha256,
            source: MapRoot::default(),
            current: MapRoot::default(),
            membership: MapRoot::default(),
            segments: MapRoot::default(),
            gram_routes: MapRoot::default(),
            term_routes: MapRoot::default(),
            compaction_due: MapRoot::default(),
            run_registry: RunRegistry::empty(),
            next_segment_id: 1,
            live_docs: 0,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{NATIVE_FORMAT, NativeRoot};
    use crate::channel_payloads::{decode_cbor_exact, encode_cbor};
    use crate::normalize::TEXT_NORMALIZER_VERSION;

    #[test]
    fn native_root_round_trips_with_run_custody_and_refuses_prior_layout() {
        let root = NativeRoot::empty([7; 32]);
        let current = encode_cbor(&root, "Native root").expect("encode current root");
        assert_eq!(
            decode_cbor_exact::<NativeRoot>(&current).expect("decode current root"),
            root
        );

        let prior = encode_cbor(
            &(
                NATIVE_FORMAT - 1,
                (TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor),
                root.allocation_scope_sha256,
                &root.source,
                &root.current,
                &root.membership,
                &root.segments,
                &root.gram_routes,
                &root.term_routes,
                &root.compaction_due,
                root.next_segment_id,
                root.live_docs,
            ),
            "prior Native root",
        )
        .expect("encode prior layout");
        assert!(decode_cbor_exact::<NativeRoot>(&prior).is_err());
    }
}
