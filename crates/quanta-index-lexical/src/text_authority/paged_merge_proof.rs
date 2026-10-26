//! Intermediate Native merge evidence shared by transition and seal oracles.

use std::collections::{BTreeMap, BTreeSet};

use crate::maintenance_merge_facts::{NativeMergeOutputFacts, NativeRunMergeFacts};

/// Transitional evidence for the changed-segment proof.
///
/// Only that proof can turn this into reportable facts. The byte map comes from
/// the transition's independent index-DAG verification, not a producer receipt.
pub(crate) struct NativeMergeProof {
    pub(crate) input: NativeRunMergeFacts,
    pub(crate) clean_doc_ids: BTreeSet<u64>,
    pub(crate) new_segment_index_bytes: BTreeMap<u64, u64>,
}

impl NativeMergeProof {
    pub(crate) fn without_merge() -> Self {
        Self {
            input: NativeRunMergeFacts {
                input_runs: 0,
                input_segments: 0,
                input_index_encoded_bytes: 0,
                output: Some(NativeMergeOutputFacts {
                    segments: 0,
                    index_encoded_bytes: 0,
                }),
            },
            clean_doc_ids: BTreeSet::new(),
            new_segment_index_bytes: BTreeMap::new(),
        }
    }
}
