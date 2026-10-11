//! Shared query resource accounting for paged postings.

use super::FilePolicy;
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use quanta_index_lq_trigram::NecessaryGramExpr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PostingSurface {
    Path,
    Content,
}

fn plan_limit(reason: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
        message: format!("lexical file authority query: {reason}"),
    }
}

#[derive(Default)]
pub(crate) struct QueryWork {
    directory_probes: u64,
    list_reads: u64,
    posting_ids: u64,
    decoded_bytes: u64,
    decoded_ids: u64,
}

impl QueryWork {
    pub(crate) fn posting_memberships(&self) -> u64 {
        self.posting_ids
    }

    /// A copy taken at the result boundary, including before a typed refusal.
    pub(crate) fn diagnostic_snapshot(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.directory_probes,
            self.list_reads,
            self.posting_ids,
            self.decoded_bytes,
            self.decoded_ids,
        )
    }

    #[cfg(test)]
    pub(crate) fn authenticated_io(&self) -> (u64, u64) {
        (self.decoded_bytes, self.list_reads)
    }

    pub(crate) fn charge_paged_probe(&mut self, policy: FilePolicy) -> Result<(), CoreError> {
        self.directory_probes = self
            .directory_probes
            .checked_add(1)
            .ok_or_else(|| plan_limit("paged route probe count overflow"))?;
        let ceiling = policy
            .query_list_reads
            .checked_mul(policy.partitions)
            .ok_or_else(|| plan_limit("paged route probe ceiling overflow"))?;
        if self.directory_probes > ceiling {
            return Err(plan_limit("paged route probe admission exceeded"));
        }
        Ok(())
    }

    pub(crate) fn charge_paged_ids(
        &mut self,
        policy: FilePolicy,
        ids: u64,
    ) -> Result<(), CoreError> {
        self.posting_ids = self
            .posting_ids
            .checked_add(ids)
            .ok_or_else(|| plan_limit("posting ID count overflow"))?;
        self.decoded_ids = self
            .decoded_ids
            .checked_add(ids)
            .ok_or_else(|| plan_limit("decoded ID count overflow"))?;
        if self.posting_ids > policy.query_posting_ids
            || self.decoded_ids > policy.query_decoded_ids
        {
            return Err(plan_limit("global query decoded ID work exceeds policy"));
        }
        Ok(())
    }

    pub(crate) fn charge_paged_io(
        &mut self,
        policy: FilePolicy,
        before: (u64, u64),
        after: (u64, u64),
    ) -> Result<(), CoreError> {
        let bytes = after
            .0
            .checked_sub(before.0)
            .ok_or_else(|| plan_limit("paged byte meter regressed"))?;
        let reads = after
            .1
            .checked_sub(before.1)
            .ok_or_else(|| plan_limit("paged read meter regressed"))?;
        self.list_reads = self
            .list_reads
            .checked_add(reads)
            .ok_or_else(|| plan_limit("paged read count overflow"))?;
        self.decoded_bytes = self
            .decoded_bytes
            .checked_add(bytes)
            .ok_or_else(|| plan_limit("paged decoded byte count overflow"))?;
        if self.list_reads > policy.query_list_reads
            || self.decoded_bytes > policy.query_decoded_bytes
        {
            return Err(plan_limit(&format!(
                "global query authenticated I/O exceeds policy: reads={}/{} bytes={}/{}",
                self.list_reads,
                policy.query_list_reads,
                self.decoded_bytes,
                policy.query_decoded_bytes
            )));
        }
        Ok(())
    }
}

pub(crate) fn validate_expression(expression: &NecessaryGramExpr) -> Result<(), CoreError> {
    const MAX_DEPTH: usize = 128;
    const MAX_NODES: usize = quanta_index_lq_trigram::MAX_TRIGRAMS_PER_QUERY;
    let mut pending = Vec::new();
    pending
        .try_reserve(1)
        .map_err(|_error| plan_limit("expression work allocation refused"))?;
    pending.push((expression, 0_usize));
    let mut nodes = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| plan_limit("expression node count overflow"))?;
        if nodes > MAX_NODES || depth > MAX_DEPTH {
            return Err(plan_limit("expression exceeds node or depth policy"));
        }
        let children = match node {
            NecessaryGramExpr::AllDocs | NecessaryGramExpr::Gram(_) => continue,
            NecessaryGramExpr::And(children) | NecessaryGramExpr::Or(children) => children,
        };
        let child_depth = depth
            .checked_add(1)
            .ok_or_else(|| plan_limit("expression depth overflow"))?;
        let scheduled = nodes
            .checked_add(pending.len())
            .and_then(|count| count.checked_add(children.len()))
            .ok_or_else(|| plan_limit("expression node count overflow"))?;
        if scheduled > MAX_NODES {
            return Err(plan_limit("expression exceeds node policy"));
        }
        pending
            .try_reserve(children.len())
            .map_err(|_error| plan_limit("expression work allocation refused"))?;
        for child in children {
            pending.push((child, child_depth));
        }
    }
    Ok(())
}

pub(crate) fn is_universal(expression: &NecessaryGramExpr) -> bool {
    match expression {
        NecessaryGramExpr::AllDocs => true,
        NecessaryGramExpr::Gram(_) => false,
        NecessaryGramExpr::And(children) => children.iter().all(is_universal),
        NecessaryGramExpr::Or(children) => children.iter().any(is_universal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_and_posting_reads_share_one_budget() {
        let mut policy = super::super::policy();
        policy.query_list_reads = 2;
        policy.query_decoded_bytes = 100;
        let mut work = QueryWork::default();
        work.charge_paged_io(policy, (0, 0), (40, 1))
            .expect("admitted metadata page");
        work.charge_paged_ids(policy, 1)
            .expect("one selected posting ID");
        work.charge_paged_io(policy, (40, 1), (80, 2))
            .expect("admitted selected page");
        assert_eq!(work.authenticated_io(), (80, 2));
        assert!(matches!(
            work.charge_paged_io(policy, (80, 2), (101, 3)),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                message,
            }) if message.ends_with("reads=3/2 bytes=101/100")
        ));
    }
}
