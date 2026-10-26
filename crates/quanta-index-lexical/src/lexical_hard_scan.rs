//! Same-attempt hard-pressure scans over immutable, adopted authority roots.

use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};

use crate::file_authority::paged::{AuthorityRoot, FileAuthorityMutation};
use crate::lexical_runs::RunSelectionProgress;
use crate::lexical_store::{
    AttemptLease, Authority, CheckpointPayload, IndependentWindowOracle, ObjectStore, ScanLane,
    WindowWitness,
};
use crate::text_authority::NativeRoot;

type LaneIdentity = (Authority, [u8; 32], u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HardScanStage {
    Prepare,
    UsePrepared,
    VerifyForSeal,
}

pub(crate) struct HardScanInputs<'a> {
    pub(crate) file: Option<(&'a AuthorityRoot, &'a FileAuthorityMutation)>,
    pub(crate) native: Option<(&'a NativeRoot, &'a [u64])>,
}

fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical hard scan: {message}"))
}

/// Debug daemon crash cut after the first window and its attempt anchor are
/// durable. Release builds contain no environment read or exit path.
#[cfg(any(test, debug_assertions))]
#[expect(
    clippy::exit,
    reason = "the integration crash cut must exit without unwinding after durable checkpoint acknowledgement"
)]
fn crash_after_first_window(ordinal: u16) {
    if ordinal == 0
        && std::env::var("QUANTA_INDEX_CRASH_POINT").as_deref()
            == Ok("after_hard_scan_first_window")
    {
        std::process::exit(86);
    }
}

#[cfg(not(any(test, debug_assertions)))]
const fn crash_after_first_window(_ordinal: u16) {}

pub(crate) fn file_root_digest(root: &AuthorityRoot) -> Result<[u8; 32], CoreError> {
    Ok(Sha256::digest(root.encode()?).into())
}

pub(crate) fn native_root_digest(root: &NativeRoot) -> Result<[u8; 32], CoreError> {
    Ok(Sha256::digest(crate::channel_payloads::encode_cbor(
        root,
        "Native hard scan base root",
    )?)
    .into())
}

impl HardScanInputs<'_> {
    fn identities(
        &self,
        store: &ObjectStore,
        policy: &LexicalMaintenancePolicy,
    ) -> Result<Vec<LaneIdentity>, CoreError> {
        let mut identities = Vec::with_capacity(2);
        if let Some((base, mutation)) = self.file
            && !mutation.clear_all
            && policy.hard_pressure(base.maintenance_snapshot(store)?)
        {
            let high = base
                .run_registry
                .next_run_id
                .checked_sub(1)
                .filter(|high| *high != 0)
                .ok_or_else(|| invalid("File hard-pressure frontier is empty"))?;
            identities.push((Authority::File, file_root_digest(base)?, high));
        }
        if let Some((base, _retired)) = self.native
            && policy.hard_pressure(base.run_registry.snapshot()?)
        {
            let high = base
                .run_registry
                .next_run_id
                .checked_sub(1)
                .filter(|high| *high != 0)
                .ok_or_else(|| invalid("Native hard-pressure frontier is empty"))?;
            identities.push((Authority::Native, native_root_digest(base)?, high));
        }
        Ok(identities)
    }

    fn prepare_window(
        &self,
        store: &ObjectStore,
        policy: &LexicalMaintenancePolicy,
        identity: LaneIdentity,
        progress: RunSelectionProgress,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<WindowWitness, CoreError> {
        match identity.0 {
            Authority::File => {
                let (base, mutation) =
                    self.file.ok_or_else(|| invalid("File scan input absent"))?;
                crate::file_authority::paged::prepare_hard_scan_window(
                    store, base, mutation, policy, progress, budget,
                )
            }
            Authority::Native => {
                let (base, retired) = self
                    .native
                    .ok_or_else(|| invalid("Native scan input absent"))?;
                crate::text_authority::prepare_hard_scan_window(
                    store, base, retired, policy, progress, budget,
                )
            }
        }
    }
}

struct OldRootOracle<'a, 'b> {
    store: &'a ObjectStore,
    inputs: &'a HardScanInputs<'b>,
    policy: &'a LexicalMaintenancePolicy,
    identities: &'a [LaneIdentity],
}

impl IndependentWindowOracle for OldRootOracle<'_, '_> {
    fn replay(
        &mut self,
        authority: Authority,
        root_digest: [u8; 32],
        start: u64,
        high: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<WindowWitness, CoreError> {
        if !self.identities.contains(&(authority, root_digest, high)) {
            return Err(invalid("independent replay base or frontier differs"));
        }
        let progress = RunSelectionProgress {
            next_scan_run_id: start,
            scan_high_water_run_id: high,
            pending_run: None,
        };
        match authority {
            Authority::File => {
                let (base, mutation) = self
                    .inputs
                    .file
                    .ok_or_else(|| invalid("File replay input absent"))?;
                crate::file_authority::paged::replay_hard_scan_window(
                    self.store,
                    base,
                    mutation,
                    self.policy,
                    progress,
                    budget,
                )
            }
            Authority::Native => {
                let (base, retired) = self
                    .inputs
                    .native
                    .ok_or_else(|| invalid("Native replay input absent"))?;
                crate::text_authority::replay_hard_scan_window(
                    self.store,
                    base,
                    retired,
                    self.policy,
                    progress,
                    budget,
                )
            }
        }
    }
}

/// Resume committed windows or require their already complete transcript.
///
/// The caller retains publication admission and base custody. This function
/// writes only the attempt checkpoint, never a source track or serving root.
pub(crate) fn prepare_or_load(
    store: &ObjectStore,
    lease: &AttemptLease,
    inputs: &HardScanInputs<'_>,
    policy: &LexicalMaintenancePolicy,
    budget: Option<&RequestBudgetV1>,
    stage: HardScanStage,
) -> Result<Option<CheckpointPayload>, CoreError> {
    let identities = inputs.identities(store, policy)?;
    if identities.is_empty() {
        return Ok(None);
    }
    let mut oracle = OldRootOracle {
        store,
        inputs,
        policy,
        identities: &identities,
    };
    let existing = match stage {
        HardScanStage::Prepare | HardScanStage::UsePrepared => {
            lease.resume_hard_scan(policy.policy_sha256(), &identities, &mut oracle, budget)?
        }
        HardScanStage::VerifyForSeal => {
            lease.load_hard_scan(policy.policy_sha256(), &identities, &mut oracle, budget)?
        }
    };
    let mut payload = match existing {
        Some(payload) => payload,
        None if stage == HardScanStage::Prepare => {
            CheckpointPayload::empty(lease.binding().clone(), policy.policy_sha256())
        }
        None => return Err(invalid("required pre-mutation checkpoint is absent")),
    };
    if stage == HardScanStage::Prepare {
        for identity in &identities {
            loop {
                if let Some(budget) = budget {
                    budget.checkpoint("lexical hard scan before window")?;
                }
                let lane = payload
                    .lanes
                    .iter()
                    .find(|lane| lane.authority == identity.0);
                if lane.is_some_and(|lane| {
                    lane.windows
                        .last()
                        .is_some_and(|window| window.round_complete)
                }) {
                    break;
                }
                let progress = lane.map_or(
                    RunSelectionProgress {
                        next_scan_run_id: 1,
                        scan_high_water_run_id: identity.2,
                        pending_run: None,
                    },
                    ScanLane::next_progress,
                );
                let witness = inputs.prepare_window(store, policy, *identity, progress, budget)?;
                let witness = payload.decorate_next_window(*identity, witness)?;
                let ordinal = witness.ordinal;
                let prior_digest = payload.digest()?;
                let (next, _digest) = lease.append_hard_scan_window(
                    payload,
                    prior_digest,
                    *identity,
                    witness,
                    &mut oracle,
                    budget,
                )?;
                payload = next;
                crash_after_first_window(ordinal);
            }
        }
    }
    payload.require_complete(&identities)?;
    Ok(Some(payload))
}

pub(crate) fn lane(payload: Option<&CheckpointPayload>, authority: Authority) -> Option<&ScanLane> {
    payload.and_then(|payload| {
        payload
            .lanes
            .iter()
            .find(|lane| lane.authority == authority)
    })
}

#[cfg(test)]
mod tests {
    use super::{HardScanInputs, HardScanStage, OldRootOracle, lane, prepare_or_load};
    use crate::lexical_allocation::DocVersionReservation;
    use crate::lexical_gc::{SharedPoolGcOutcome, collect_shared_pool};
    use crate::lexical_store::{
        AttemptBinding, AttemptLease, Authority, CheckpointPayload, ObjectStore,
    };
    use crate::publication_faults::{Cut, Side, with_error_at};
    use crate::text_authority::verify_transition_for_seal_with_policy;
    use crate::text_authority::{AddedTextDoc, NativeAuthority, NativeRoot, NativeSourceEdit};
    use crate::text_authority::{publish_delta, publish_delta_prepared_with_policy};
    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SourceFileCoverage,
        SourceFileKey, SourceFileRevision, SymbolCoverage, SymbolNameSourcePolicyV1,
    };
    use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
    use quanta_index_core::request_budget::REQUEST_CANCELLED_CODE;
    use quanta_index_core::{CoreError, RequestBudgetV1};
    use sha2::{Digest as _, Sha256};

    const SCOPE: [u8; 32] = [9; 32];

    fn binding(generation: u64) -> Result<AttemptBinding, Box<dyn std::error::Error>> {
        let byte = u8::try_from(generation)?;
        Ok(AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [byte; 32],
            source_event_sha256: [byte; 32],
            target_generation: generation,
            base_root_sha256: (generation > 1).then_some([byte.wrapping_sub(1); 32]),
            operations_sha256: [byte; 32],
            format: 1,
        })
    }

    fn source(id: u64) -> Result<SourceFileKey, Box<dyn std::error::Error>> {
        Ok(SourceFileKey {
            source_repo_id: RepoId::new("source-repo")?,
            repo_relative_path: RepoRelativePath::new(format!("src/doc-{id}.rs")),
        })
    }

    fn text(id: u64) -> String {
        if id == 1 {
            format!("needle{id} {}", "filler ".repeat(64))
        } else if id <= 63 {
            format!("needle{id} {}", "filler ".repeat(512))
        } else {
            format!("needle{id} tiny")
        }
    }

    fn edit(id: u64, text: &str) -> Result<NativeSourceEdit, Box<dyn std::error::Error>> {
        Ok(NativeSourceEdit::Upsert(SourceFileCoverage {
            source: SourceFileRevision {
                file: source(id)?,
                revision_id: RevisionId::new(format!("source-r{id}"))?,
                source_sha256: Sha256::digest(text.as_bytes()).into(),
            },
            language: quanta_index_contract::lex::LanguageCode::new("rust")?,
            producer_policy_sha256: [1; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: [2; 32],
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 0 },
        }))
    }

    fn document(id: u64, text: String) -> Result<AddedTextDoc, Box<dyn std::error::Error>> {
        Ok(AddedTextDoc {
            doc_id: id,
            source_file: source(id)?,
            candidate_id: format!("doc-{id}"),
            text,
        })
    }

    fn checkpoint_prefix(
        store: &ObjectStore,
        lease: &AttemptLease,
        inputs: &HardScanInputs<'_>,
        policy: &LexicalMaintenancePolicy,
    ) -> Result<CheckpointPayload, Box<dyn std::error::Error>> {
        let identities = inputs.identities(store, policy)?;
        let mut oracle = OldRootOracle {
            store,
            inputs,
            policy,
            identities: &identities,
        };
        Ok(lease
            .load_hard_scan(policy.policy_sha256(), &identities, &mut oracle, None)?
            .ok_or("durable checkpoint prefix missing")?)
    }

    #[test]
    fn native_hard_scan_restarts_from_durable_window_after_fault_and_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let dir = tempfile::tempdir()?;
        let store = ObjectStore::open(dir.path())?;
        let mut base: Option<NativeRoot> = None;
        for id in 1_u64..=65 {
            let body = text(id);
            let inherited = base
                .as_ref()
                .map_or_else(Vec::new, NativeRoot::root_objects);
            let lease = store.begin_attempt(&binding(id)?, &inherited)?;
            let (next, _) = publish_delta(
                &store,
                &lease,
                base.as_ref(),
                std::slice::from_ref(&edit(id, &body)?),
                &[],
                std::slice::from_ref(&document(id, body)?),
                false,
                ManifestGeneration::new(id),
                SCOPE,
            )?;
            lease.finish_published(&next.root_objects())?;
            base = Some(next);
        }
        let base = base.ok_or("missing old Native root")?;
        check_eq!(base.run_registry.runs.entries, 65);
        let run_cost = |id| -> Result<u64, Box<dyn std::error::Error>> {
            Ok(base
                .run_registry
                .candidate_by_id(&store, id, None)?
                .ok_or("missing old run")?
                .index_encoded_bytes)
        };
        let tail_pair = run_cost(64)?
            .checked_add(run_cost(65)?)
            .ok_or("tail cost overflow")?;
        let pair_cap = run_cost(1)?.max(tail_pair);
        check!(
            run_cost(1)?
                .checked_add(run_cost(64)?)
                .is_some_and(|sum| sum > pair_cap)
        );
        check!(tail_pair <= pair_cap);
        for id in 2_u64..=63 {
            check!(
                run_cost(id)? > pair_cap,
                "prefix run {id} must be ineligible"
            );
        }
        let policy = LexicalMaintenancePolicy::new(
            2,
            65,
            1,
            u64::MAX,
            8,
            pair_cap,
            64 * 1024 * 1024,
            60_000,
        )?;
        check!(policy.hard_pressure(base.run_registry.snapshot()?));
        let old_query = NativeAuthority::open(store.clone(), base.clone())?;
        let query_budget = RequestBudgetV1::unbounded();
        check_eq!(
            old_query.intersect_terms(&["needle64"], true, &query_budget)?,
            vec![64]
        );

        let target_binding = binding(66)?;
        let lease = store.begin_attempt(&target_binding, &base.root_objects())?;
        let inputs = HardScanInputs {
            file: None,
            native: Some((&base, &[])),
        };
        let (failed, fired) =
            with_error_at(Cut::HardScanCheckpointDirectorySync, Side::After, 1, || {
                prepare_or_load(
                    &store,
                    &lease,
                    &inputs,
                    &policy,
                    None,
                    HardScanStage::Prepare,
                )
            });
        check!(
            fired,
            "first window must reach the real durable sidecar write"
        );
        check!(
            failed.is_err(),
            "post-sync fault must surface to the caller"
        );
        let prefix = checkpoint_prefix(&store, &lease, &inputs, &policy)?;
        let prefix_lane = lane(Some(&prefix), Authority::Native).ok_or("Native prefix absent")?;
        check_eq!(prefix_lane.windows.len(), 1);
        let first = prefix_lane.windows.first().ok_or("first window absent")?;
        check_eq!(first.candidates.len(), 32);
        check_eq!(first.start_run_id, 1);
        check_eq!(first.next_run_id, 33);
        check!(!first.round_complete);
        check_eq!(
            first
                .candidates
                .first()
                .ok_or("first candidate absent")?
                .run_id,
            1
        );
        check_eq!(
            first
                .candidates
                .last()
                .ok_or("last candidate absent")?
                .run_id,
            32
        );
        check_eq!(
            old_query.intersect_terms(&["needle64"], true, &query_budget)?,
            vec![64]
        );
        // No generation directory is supplied to this collection. The old
        // root remains reachable solely through the pending attempt's
        // inherited-root adoption while its checkpoint is incomplete.
        let gc = collect_shared_pool(&store, &[], store.gc_epoch()?, None)?;
        check!(matches!(gc, SharedPoolGcOutcome::Completed { marked, .. } if marked > 0));
        check_eq!(
            old_query.intersect_terms(&["needle64"], true, &query_budget)?,
            vec![64]
        );

        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        let interrupted = prepare_or_load(
            &store,
            &lease,
            &inputs,
            &policy,
            Some(&cancelled),
            HardScanStage::Prepare,
        )
        .expect_err("cancelled retry must not publish a checkpoint window");
        check!(
            matches!(interrupted, CoreError::Typed { code, .. } if code == REQUEST_CANCELLED_CODE)
        );
        check_eq!(checkpoint_prefix(&store, &lease, &inputs, &policy)?, prefix);
        drop(old_query);
        drop(lease);
        drop(store);

        let reopened = ObjectStore::open_existing(dir.path())?;
        let retry = reopened.begin_attempt(&target_binding, &base.root_objects())?;
        let (failed_again, fired_again) =
            with_error_at(Cut::HardScanCheckpointDirectorySync, Side::After, 1, || {
                prepare_or_load(
                    &reopened,
                    &retry,
                    &inputs,
                    &policy,
                    Some(&RequestBudgetV1::unbounded()),
                    HardScanStage::Prepare,
                )
            });
        check!(
            fired_again,
            "second window must reach a durable sidecar write"
        );
        check!(failed_again.is_err());
        let prefix_two = checkpoint_prefix(&reopened, &retry, &inputs, &policy)?;
        let prefix_two_lane =
            lane(Some(&prefix_two), Authority::Native).ok_or("second Native prefix absent")?;
        check_eq!(prefix_two_lane.windows.len(), 2);
        check_eq!(prefix_two_lane.windows.first(), prefix_lane.windows.first());
        let second = prefix_two_lane
            .windows
            .get(1)
            .ok_or("second window absent")?;
        check_eq!(second.start_run_id, 33);
        check_eq!(second.next_run_id, 65);
        check!(!second.round_complete);
        check_eq!(
            second
                .candidates
                .iter()
                .map(|candidate| candidate.run_id)
                .collect::<Vec<_>>(),
            (33_u64..=64).collect::<Vec<_>>()
        );
        drop(retry);
        drop(reopened);

        let reopened = ObjectStore::open_existing(dir.path())?;
        let retry = reopened.begin_attempt(&target_binding, &base.root_objects())?;
        let resumed = prepare_or_load(
            &reopened,
            &retry,
            &inputs,
            &policy,
            Some(&RequestBudgetV1::unbounded()),
            HardScanStage::Prepare,
        )?
        .ok_or("complete Native checkpoint absent")?;
        let resumed_lane = lane(Some(&resumed), Authority::Native).ok_or("Native lane absent")?;
        check_eq!(resumed_lane.windows.len(), 3);
        check_eq!(
            resumed_lane.windows.get(..2),
            prefix_two_lane.windows.get(..2)
        );
        let third = resumed_lane.windows.get(2).ok_or("third window absent")?;
        check_eq!(third.start_run_id, 65);
        check_eq!(third.next_run_id, 1);
        check!(third.round_complete);
        check_eq!(
            third
                .candidates
                .iter()
                .map(|candidate| candidate.run_id)
                .collect::<Vec<_>>(),
            vec![65]
        );
        check_eq!(
            checkpoint_prefix(&reopened, &retry, &inputs, &policy)?,
            resumed
        );
        let fast = prepare_or_load(
            &reopened,
            &retry,
            &inputs,
            &policy,
            Some(&RequestBudgetV1::unbounded()),
            HardScanStage::UsePrepared,
        )?
        .ok_or("fast resumed checkpoint absent")?;
        check_eq!(fast, resumed);
        let sealed = prepare_or_load(
            &reopened,
            &retry,
            &inputs,
            &policy,
            None,
            HardScanStage::VerifyForSeal,
        )?
        .ok_or("seal replay checkpoint absent")?;
        check_eq!(sealed, resumed);

        let next_text = text(66);
        let next_edit = edit(66, &next_text)?;
        let next_doc = document(66, next_text)?;
        let (target, _) = publish_delta_prepared_with_policy(
            &reopened,
            &retry,
            Some(&base),
            std::slice::from_ref(&next_edit),
            &[],
            std::slice::from_ref(&next_doc),
            false,
            ManifestGeneration::new(66),
            SCOPE,
            None,
            Some(&policy),
            Some(resumed_lane),
        )?;
        policy.admit_target(
            base.run_registry.snapshot()?,
            target.run_registry.snapshot()?,
        )?;
        check_eq!(target.run_registry.runs.entries, 65);
        check!(
            target
                .run_registry
                .candidate_by_id(&reopened, 64, None)?
                .is_none()
        );
        check!(
            target
                .run_registry
                .candidate_by_id(&reopened, 65, None)?
                .is_none()
        );
        let _seal_proof = verify_transition_for_seal_with_policy(
            &reopened,
            &target,
            Some(&base),
            &DocVersionReservation {
                first: 66,
                count: 1,
                scope_sha256: SCOPE,
            },
            std::slice::from_ref(&next_edit),
            &[],
            std::slice::from_ref(&next_doc),
            false,
            base.next_segment_id,
            None,
            Some(&policy),
            Some(resumed_lane),
        )?;
        let target_query = NativeAuthority::open(reopened, target.clone())?;
        check_eq!(
            target_query.intersect_terms(&["needle64"], true, &query_budget)?,
            vec![64]
        );
        check_eq!(
            target_query
                .read_document(64, &query_budget)?
                .map(|document| document.indexed_text),
            Some(text(64))
        );
        retry.finish_published(&target.root_objects())?;
        Ok(())
    }
}
