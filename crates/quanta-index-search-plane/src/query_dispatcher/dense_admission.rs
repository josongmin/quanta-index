//! The dense lane under exact filters: over-fetch, admit every candidate
//! through the lexical plan, refill (QI-BB-018 보완 #3).
//!
//! The classes and the refill policy are core's
//! ([`HybridFilterPlanV1`], [`HybridOrchestratorPolicy`]); this module is the
//! loop that runs one dense lane under them, shared by the hybrid and
//! hybrid-seed routes so the two cannot drift on what a filtered dense lane
//! is.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::QueryConstraintSetV1;
use quanta_index_core::{
    CoreError, DenseAdmissionOutcomeV1, HybridFilterPlanV1, HybridOrchestratorPolicy,
    LexicalSearcher, RequestBudgetV1, dense_admission_round_outcome_v1,
};

use super::execution_trace::LaneExecutionRecorderV1;

/// One dense lane after admission.
pub(super) struct AdmittedDenseLaneV1<T> {
    /// The lane's rows in the engine's order: every fetched row when no
    /// filter needed evaluation, otherwise the admitted rows of the last
    /// fetch, cut to the target depth.
    pub(super) rows: Vec<T>,
    /// Rows the last fetch returned: what the lane examined.
    pub(super) examined: usize,
    /// Rows of the last fetch the filters admitted, before the cut.
    pub(super) admitted: usize,
    pub(super) outcome: DenseAdmissionOutcomeV1,
}

impl<T> AdmittedDenseLaneV1<T> {
    /// The trace detail under `key` (`hybrid.dense_admission`, ...):
    /// `not_needed` when no filter was evaluated per candidate, otherwise
    /// how the loop ended and what it examined and admitted.
    pub(super) fn trace_detail(&self, key: &str) -> String {
        match self.outcome {
            DenseAdmissionOutcomeV1::NotNeeded => {
                format!("{key}={}", DenseAdmissionOutcomeV1::NotNeeded.as_str())
            }
            outcome @ (DenseAdmissionOutcomeV1::Filled
            | DenseAdmissionOutcomeV1::Exhausted
            | DenseAdmissionOutcomeV1::Capped) => format!(
                "{key}={}; examined={}; admitted={}",
                outcome.as_str(),
                self.examined,
                self.admitted
            ),
        }
    }
}

/// Run one dense lane under `plan`.
///
/// `fetch(size)` is the engine's ranked search for `size` rows under the
/// pushed-down constraints; `id_of` names a row's candidate identity;
/// `target` is the depth an unfiltered lane has. Without exact filters the
/// lane is one fetch of `target`. With them, every fetched candidate is
/// admitted or excluded through [`LexicalSearcher::admitted_candidates`]
/// under the plan's filter-only query (each identity is asked about once),
/// and the lane refetches at the sizes
/// [`HybridOrchestratorPolicy::next_dense_admission_fetch`] gives until the
/// admitted rows reach `target`, the engine returns fewer rows than asked,
/// or the ceiling was fetched; the outcome names which.
///
/// Invocation truth (W10-R1): the caller's `fetch` closure records each
/// fetch that reaches the semantic backend (a short-circuited closure
/// records nothing); this loop records each filter round that reaches the
/// lexical backend through `admitted_candidates`.
pub(super) fn admit_dense_lane_v1<T>(
    plan: &HybridFilterPlanV1,
    lexical: &dyn LexicalSearcher,
    constraints: &QueryConstraintSetV1,
    target: u32,
    budget: &RequestBudgetV1,
    execution: &LaneExecutionRecorderV1,
    id_of: impl Fn(&T) -> &str,
    mut fetch: impl FnMut(u32) -> Result<Vec<T>, CoreError>,
) -> Result<AdmittedDenseLaneV1<T>, CoreError> {
    let Some(admission) = plan.admission_query() else {
        let rows = fetch(target)?;
        let examined = rows.len();
        return Ok(AdmittedDenseLaneV1 {
            rows,
            examined,
            admitted: examined,
            outcome: DenseAdmissionOutcomeV1::NotNeeded,
        });
    };
    let depth = usize::try_from(target)
        .map_err(|err| CoreError::InvalidContract(format!("dense lane depth overflow: {err}")))?;
    let mut verdicts: BTreeMap<String, bool> = BTreeMap::new();
    let mut fetch_size = target;
    loop {
        let fetched = fetch(fetch_size)?;
        let examined = fetched.len();
        let unknown = fetched
            .iter()
            .map(&id_of)
            .filter(|id| !verdicts.contains_key(*id))
            .map(str::to_owned)
            .collect::<BTreeSet<String>>();
        if !unknown.is_empty() {
            execution.record_lexical_invocation();
            let admitted = lexical.admitted_candidates(admission, constraints, &unknown, budget)?;
            for id in unknown {
                let verdict = admitted.contains(&id);
                let _new_identity = verdicts.insert(id, verdict);
            }
        }
        let mut rows = fetched
            .into_iter()
            .filter(|row| matches!(verdicts.get(id_of(row)), Some(true)))
            .collect::<Vec<T>>();
        let admitted = rows.len();
        let ended = dense_admission_round_outcome_v1(admitted, examined, fetch_size, target);
        let next_fetch = HybridOrchestratorPolicy::next_dense_admission_fetch(fetch_size);
        let outcome = match (ended, next_fetch) {
            (Some(outcome), _) => outcome,
            (None, None) => DenseAdmissionOutcomeV1::Capped,
            (None, Some(next)) => {
                fetch_size = next;
                continue;
            }
        };
        // The lane is as deep as an unfiltered one, never deeper.
        rows.truncate(depth);
        return Ok(AdmittedDenseLaneV1 {
            rows,
            examined,
            admitted,
            outcome,
        });
    }
}
