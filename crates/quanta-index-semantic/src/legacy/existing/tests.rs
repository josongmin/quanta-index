//! Pure owner-codec admission regressions, not a sealed dataset or permission.
use super::*;
use crate::{
    generation_contract::GenerationContract,
    legacy::sealed::SemanticSealedManifestV1,
    manifest::{SemanticManifest, VectorIndexSealV1},
};
use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits},
};
use sha2::{Digest as _, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const ONE_MIB: u64 = 1024 * 1024;
const ROOT: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

struct Memory {
    limit: u64,
    live: AtomicU64,
    largest: AtomicU64,
    calls: AtomicU64,
    bad_release: AtomicBool,
}
impl MemoryLeaseOwner for Memory {
    fn release(&self, bytes: u64) {
        if self
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_sub(bytes)
            })
            .is_err()
        {
            self.bad_release.store(true, Ordering::SeqCst);
        }
    }
}
struct Scope(Arc<Memory>);
impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self.0.calls.fetch_add(1, Ordering::SeqCst);
        let _largest = self.0.largest.fetch_max(bytes, Ordering::SeqCst);
        let _live = self
            .0
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes)
                    .filter(|total| *total <= self.0.limit)
            })
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}
fn work(limit: u64, request: RequestBudgetV1) -> (WorkBudget, Arc<Memory>) {
    let memory = Arc::new(Memory {
        limit,
        live: AtomicU64::new(0),
        largest: AtomicU64::new(0),
        calls: AtomicU64::new(0),
        bad_release: AtomicBool::new(false),
    });
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 1000,
            writes: 0,
            bytes: ONE_MIB,
            mutations: 0,
        },
        request,
    );
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(memory.clone())))));
    (budget, memory)
}
fn expected() -> TestResult<GenerationSnapshot> {
    Ok(GenerationSnapshot {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("revision")?,
        track: SearchPlaneTrackKind::Semantic,
        manifest_generation: ManifestGeneration::new(1),
        manifest_digest: "d".into(),
    })
}
fn slices(data: &[Vec<u8>; 4]) -> [&[u8]; 4] {
    let [marker, seal, scope, contract] = data;
    [marker, seal, scope, contract]
}
fn released(memory: &Memory) -> TestResult {
    if memory.live.load(Ordering::SeqCst) != 0 || memory.bad_release.load(Ordering::SeqCst) {
        return Err("owner-codec allocation did not release with its value".into());
    }
    Ok(())
}

fn malformed_array() -> [Vec<u8>; 4] {
    // Independent literal CBOR row [1,"d",[0,[0;32]],[0,[0;32]],array(MAX)].
    // On current64-bit Serde this untrusted count advertises up to1MiB of Vec
    // backing before the missing first element is read. It is not a dataset.
    let mut seal = vec![0x85, 0x01, 0x61, b'd', 0x82, 0x00, 0x98, 0x20];
    seal.extend([0_u8; 32]);
    seal.extend([0x82, 0x00, 0x98, 0x20]);
    seal.extend([0_u8; 32]);
    seal.extend([0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    [vec![b'd'], seal, vec![0x80], vec![0x80]]
}

fn valid_codec_data() -> TestResult<[Vec<u8>; 4]> {
    // Existing owner encoders prepare valid metadata only. The independent
    // oracle is allocation lifetime/refusal, not a round-trip wire assertion.
    let contract = GenerationContract {
        format_version: 5,
        mode: BatchIngestMode::ReplaceGeneration,
        base_generation: None,
        model_id: "unit-model".into(),
        model_version: Some("v1".into()),
        dimension: 2,
        distance_metric: "cosine".into(),
        normalization: "none".into(),
        policy_digest: ROOT.into(),
        view_policy_digest: None,
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
    };
    let scope = SemanticManifest {
        format_version: 14,
        repo_id: "repo".into(),
        revision_id: "revision".into(),
        generation: 1,
        manifest_digest: "d".into(),
        model_id: "unit-model".into(),
        model_version: Some("v1".into()),
        dimension: 2,
        distance_metric: "cosine".into(),
        normalization: "none".into(),
        policy_digest: ROOT.into(),
        view_policy_digest: None,
        row_count: 0,
        semantic_row_root_digest: ROOT.into(),
        built_at_unix_nanos: 1,
        present_corpora: Vec::new(),
        required_corpora: Vec::new(),
        card_schema_versions: Vec::new(),
        render_policy_digests: Vec::new(),
        corpus_policy_digest: None,
        cluster_membership_root_digest: ROOT.into(),
        cluster_membership_cluster_count: 0,
        cluster_membership_member_row_count: 0,
        vector_index: VectorIndexSealV1 {
            mode: "exact".into(),
            library: "lancedb".into(),
            library_version: "unit-fixture".into(),
            index_min_rows: 1,
            ann: None,
        },
    };
    let scope_bytes = scope.encode()?;
    let contract_bytes = contract.encode()?;
    let seal = SemanticSealedManifestV1 {
        format_version: 1,
        manifest_digest: "d".into(),
        scope_manifest: (
            u64::try_from(scope_bytes.len())?,
            Sha256::digest(&scope_bytes).into(),
        ),
        build_contract: (
            u64::try_from(contract_bytes.len())?,
            Sha256::digest(&contract_bytes).into(),
        ),
        artifacts: Vec::new(),
    };
    let seal_bytes = crate::codec::encode(&seal.to_row(), "codec admission unit row")?;
    Ok([vec![b'd'], seal_bytes, scope_bytes, contract_bytes])
}

#[test]
fn semantic_cbor_original_memory_refuses_before_malformed_header_decode() -> TestResult {
    let data = malformed_array();
    let (mut budget, memory) = work(
        ONE_MIB.checked_sub(1).ok_or("unit bound underflow")?,
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let result = with_admitted_decoded_data(
        Path::new("codec-unit"),
        &expected()?,
        slices(&data),
        &mut budget,
        |_, _, _| {
            Err::<(), _>(
                CoreError::InvalidContract("malformed header reached consumer".into()).into(),
            )
        },
    );
    if !matches!(
        result,
        Err(Failure::AdmissionOrIo(IndexError::ResourceLimited))
    ) {
        return Err(
            "small original memory did not refuse before canonical malformed decode".into(),
        );
    }
    if memory.largest.load(Ordering::SeqCst) < ONE_MIB {
        return Err(
            "malformed array was not covered by even Serde's known1MiB preallocation".into(),
        );
    }
    released(&memory)
}

#[test]
fn semantic_cbor_sufficient_memory_preserves_canonical_malformed_refusal() -> TestResult {
    let data = malformed_array();
    let (mut budget, memory) = work(
        32 * ONE_MIB,
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let result = with_admitted_decoded_data(
        Path::new("codec-unit"),
        &expected()?,
        slices(&data),
        &mut budget,
        |_, _, _| {
            Err::<(), _>(
                CoreError::InvalidContract("malformed header reached consumer".into()).into(),
            )
        },
    );
    if !matches!(
        result,
        Err(Failure::Canonical(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        }))
    ) {
        return Err("funded malformed CBOR did not retain canonical owner refusal".into());
    }
    released(&memory)
}

#[test]
fn semantic_cbor_decode_grant_lives_through_original_budget_consumer() -> TestResult {
    let data = valid_codec_data()?;
    let (mut budget, memory) = work(
        32 * ONE_MIB,
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let original = std::ptr::from_ref(&budget);
    with_admitted_decoded_data(
        Path::new("codec-unit"),
        &expected()?,
        slices(&data),
        &mut budget,
        |files, roots, supplied| {
            if !std::ptr::eq(original, std::ptr::from_ref(supplied))
                || memory.live.load(Ordering::SeqCst) < ONE_MIB
            {
                return Err(CoreError::InvalidContract(
                    "consumer lost original budget or live decoder allocation".into(),
                )
                .into());
            }
            if !files.is_empty()
                || roots.row_root_digest != ROOT
                || roots.membership_root_digest != ROOT
            {
                return Err(CoreError::InvalidContract(
                    "unit metadata differs from the fixed expected roots".into(),
                )
                .into());
            }
            Ok(())
        },
    )?;
    released(&memory)
}

#[test]
fn semantic_cbor_cancelled_original_refuses_before_allocation() -> TestResult {
    let data = malformed_array();
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    request.cancel_handle().cancel();
    let (mut budget, memory) = work(32 * ONE_MIB, request);
    let result = with_admitted_decoded_data(
        Path::new("codec-unit"),
        &expected()?,
        slices(&data),
        &mut budget,
        |_, _, _| Ok(()),
    );
    if !matches!(result, Err(Failure::AdmissionOrIo(IndexError::Cancelled)))
        || memory.calls.load(Ordering::SeqCst) != 0
    {
        return Err("cancelled original was replaced or granted new allocation".into());
    }
    released(&memory)
}

mod physical;
