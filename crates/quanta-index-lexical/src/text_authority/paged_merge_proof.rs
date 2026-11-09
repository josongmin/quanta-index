//! Intermediate Native merge evidence shared by transition and seal oracles.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use crate::lexical_store::{BlobRef, MapRoot, ObjectRef};
use crate::maintenance_merge_facts::{NativeMergeOutputFacts, NativeRunMergeFacts};

const ROUTE_WITNESS_SEGMENT_CHARGE: usize = 256;
const ROUTE_WITNESS_PAGE_CHARGE: usize = 512;

pub(super) type VerifiedRoutePages = BTreeMap<ObjectRef, VerifiedRoutePage>;

/// Authenticated route-to-page assignments from the transition walk. Retains
/// one commitment per physical posting page instead of one reference per key.
pub(super) struct RouteWitness {
    grams: MapRoot,
    terms: MapRoot,
    segments: BTreeMap<u64, VerifiedRoutePages>,
    charged_bytes: usize,
}

pub(super) struct VerifiedRoutePage {
    pub(super) reference: BlobRef,
    keys: Sha256,
    count: usize,
}

impl RouteWitness {
    pub(super) fn new(grams: &MapRoot, terms: &MapRoot) -> Self {
        Self {
            grams: grams.clone(),
            terms: terms.clone(),
            segments: BTreeMap::new(),
            charged_bytes: 0,
        }
    }

    pub(super) fn matches(&self, grams: &MapRoot, terms: &MapRoot) -> bool {
        self.grams == *grams && self.terms == *terms
    }

    pub(super) fn pages(&self, segment: u64) -> Option<&VerifiedRoutePages> {
        self.segments.get(&segment)
    }

    pub(super) fn add(
        &mut self,
        segment: u64,
        key: &[u8],
        reference: &BlobRef,
        limit: usize,
    ) -> Result<bool, CoreError> {
        if let Some(page) = self
            .segments
            .get_mut(&segment)
            .and_then(|pages| pages.get_mut(&reference.root))
        {
            page.append(reference, key)?;
            return Ok(true);
        }
        let charge = usize::from(!self.segments.contains_key(&segment))
            * ROUTE_WITNESS_SEGMENT_CHARGE
            + ROUTE_WITNESS_PAGE_CHARGE;
        let Some(total) = self
            .charged_bytes
            .checked_add(charge)
            .filter(|total| *total <= limit)
        else {
            return Ok(false);
        };
        let mut page = VerifiedRoutePage {
            reference: reference.clone(),
            keys: Sha256::new(),
            count: 0,
        };
        page.append(reference, key)?;
        let _previous = self
            .segments
            .entry(segment)
            .or_default()
            .insert(reference.root.clone(), page);
        self.charged_bytes = total;
        Ok(true)
    }
}

impl VerifiedRoutePage {
    fn append(&mut self, reference: &BlobRef, key: &[u8]) -> Result<(), CoreError> {
        if self.reference != *reference {
            return Err(corrupt("route aliases disagree on posting reference"));
        }
        append_key(&mut self.keys, key)?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| corrupt("route count overflows"))?;
        Ok(())
    }

    pub(super) fn verify_keys(&self, sorted_keys: &[Vec<u8>]) -> Result<(), CoreError> {
        let mut keys = Sha256::new();
        for key in sorted_keys {
            append_key(&mut keys, key)?;
        }
        if sorted_keys.len() != self.count || keys.finalize() != self.keys.clone().finalize() {
            return Err(corrupt(
                "posting page keys differ from authenticated route assignments",
            ));
        }
        Ok(())
    }
}

fn append_key(hash: &mut Sha256, key: &[u8]) -> Result<(), CoreError> {
    let length = u64::try_from(key.len()).map_err(|_| corrupt("route key width overflows"))?;
    hash.update(length.to_be_bytes());
    hash.update(key);
    Ok(())
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("lexical: Native route witness: {reason}"),
    }
}

/// Transitional evidence for the changed-segment proof.
///
/// Only that proof can turn this into reportable facts. The byte map comes from
/// the transition's independent index-DAG verification, not a producer receipt.
pub(crate) struct NativeMergeProof {
    pub(crate) input: NativeRunMergeFacts,
    pub(crate) clean_doc_ids: BTreeSet<u64>,
    pub(crate) new_segment_index_bytes: BTreeMap<u64, u64>,
    pub(super) routes: Option<RouteWitness>,
}

impl NativeMergeProof {
    pub(super) fn without_merge(routes: Option<RouteWitness>) -> Self {
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
            routes,
        }
    }
}
