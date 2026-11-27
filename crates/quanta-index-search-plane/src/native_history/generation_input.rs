//! Validate each original producer output before adding it to the published
//! History member's immutable closure. The Legacy and Native routes share the
//! ingest DTOs and digest codec.

use super::{SourceInputData, build::source_input, history_work_error, invalid};
use quanta_index_contract::{
    FileContributorIngestBatch, HistoryIngestBatch, RepoCommitRecencyIngestBatch,
    index::{HistoryGenerationInputV1, NativeHistoryInitialInputV1},
};
use quanta_index_core::{
    CoreError, IngestBatchBodyV1,
    index::{
        AttemptContext, IndexError, MemoryLifetime, ObjectRef, StoredObject, WorkBudget, WorkUsage,
    },
};
use quanta_index_ipc::{
    CanonicalBatchDigestProgressError, canonical_batch_digest_with_progress_v1,
    matches_batch_digest_token_v1,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn decode_closure(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<HistoryGenerationInputV1, CoreError> {
    const MAX_CLOSURE_BYTES: usize = 4096;
    if stored.payload.len() > MAX_CLOSURE_BYTES {
        return Err(invalid(
            "History generation input exceeds its fixed metadata bound",
        ));
    }
    let value = source_input::decode_history_object_allow_children::<HistoryGenerationInputV1>(
        reference, stored, budget,
    )?
    .value;
    value
        .verify_children_v1(stored.registration.children.as_slice())
        .map_err(invalid)?;
    Ok(value)
}

pub(super) fn load_verified(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<HistoryIngestBatch>, CoreError> {
    let stored = context
        .objects
        .get(reference, context.budget)
        .map_err(|error| history_work_error(context.budget, error))?;
    let closure = decode_closure(reference, &stored, context.budget)?;
    drop(stored);
    load_children_verified(closure, context)
}

pub(super) fn verify_initial_with_get(
    input: NativeHistoryInitialInputV1,
    mut get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
) -> Result<SourceInputData<HistoryIngestBatch>, CoreError> {
    if input.wire_version != NativeHistoryInitialInputV1::WIRE_VERSION {
        return Err(invalid("Native History initial input version is invalid"));
    }
    let original = get(input.original, budget).map_err(|error| history_work_error(budget, error))?;
    let original = source_input::decode_history_batch_object(input.original, &original, budget)?;
    let recency = input.repo_commit_recency.map(|reference| {
        let stored = get(reference, budget).map_err(|error| history_work_error(budget, error))?;
        if !stored.registration.children.is_empty() {
            return Err(invalid("History derived output has unexpected children"));
        }
        source_input::decode_history_object_allow_children::<RepoCommitRecencyIngestBatch>(
            reference, &stored, budget,
        )
    }).transpose()?;
    verify_recency(&original.value, recency.as_ref().map(|data| &data.value), budget)?;
    drop(recency);
    let contributor = input.file_contributor.map(|reference| {
        let stored = get(reference, budget).map_err(|error| history_work_error(budget, error))?;
        source_input::decode_history_contributor_object(reference, &stored, budget)
    }).transpose()?;
    verify_contributors(&original.value, contributor.as_ref().map(|data| &data.value), budget)?;
    Ok(original)
}

fn load_children_verified(
    closure: HistoryGenerationInputV1,
    context: &mut AttemptContext<'_>,
) -> Result<SourceInputData<HistoryIngestBatch>, CoreError> {
    let input = source_input::load_history_batch_object(closure.original, context)?;
    let recency = closure
        .repo_commit_recency
        .map(|child| source_input::load_history_object::<RepoCommitRecencyIngestBatch>(child, context))
        .transpose()?;
    verify_recency(
        &input.value,
        recency.as_ref().map(|data| &data.value),
        context.budget,
    )?;
    drop(recency);
    let contributor = closure
        .file_contributor
        .map(|child| source_input::load_history_contributor_object(child, context))
        .transpose()?;
    verify_contributors(
        &input.value,
        contributor.as_ref().map(|data| &data.value),
        context.budget,
    )?;
    Ok(input)
}

fn require_scope_and_digest<T: IngestBatchBodyV1 + Serialize>(
    source: &HistoryIngestBatch,
    output: &T,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    if output.repo_id() != &source.repo_id
        || output.revision_id() != &source.revision_id
        || output.generation() != source.generation
    {
        return Err(invalid(
            "History derived output differs from the original generation",
        ));
    }
    let digest = canonical_batch_digest_with_progress_v1(output, &mut |bytes| {
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })
    })
    .map_err(|error| match error {
        CanonicalBatchDigestProgressError::Progress(error) => history_work_error(budget, error),
        CanonicalBatchDigestProgressError::Encode(error) => CoreError::Storage(error.to_string()),
    })?;
    if !matches_batch_digest_token_v1(output.batch_digest(), &digest) {
        return Err(invalid(
            "History derived output differs from its canonical batch digest",
        ));
    }
    Ok(())
}

fn verify_recency(
    source: &HistoryIngestBatch,
    output: Option<&RepoCommitRecencyIngestBatch>,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    let mut latest = None;
    for commit in &source.commits {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        latest = latest.max(Some(commit.committer_time_ms));
    }
    match (latest, output) {
        (None, None) => Ok(()),
        (Some(expected), Some(output)) => {
            require_scope_and_digest(source, output, budget)?;
            let [entry] = output.entries.as_slice() else {
                return Err(invalid(
                    "History commit-recency output must have one exact repository entry",
                ));
            };
            if entry.source_repo_id != source.repo_id || entry.latest_committer_time_ms != expected
            {
                return Err(invalid(
                    "History commit-recency output differs from original commits",
                ));
            }
            Ok(())
        }
        _ => Err(invalid(
            "History commit-recency output has invalid presence",
        )),
    }
}

type Contributor<'a> = (&'a str, Option<&'a str>, Option<&'a str>);

fn verify_contributors(
    source: &HistoryIngestBatch,
    output: Option<&FileContributorIngestBatch>,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    let output = match (source.diff_hunks.is_empty(), output) {
        (true, None) => return Ok(()),
        (false, Some(output)) => output,
        _ => {
            return Err(invalid(
                "History file-contributor output has invalid presence",
            ));
        }
    };
    require_scope_and_digest(source, output, budget)?;
    let records = source
        .commits
        .len()
        .checked_add(source.diff_hunks.len())
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| invalid("History contributor census overflow"))?;
    let backing = records
        .checked_mul(256)
        .and_then(|size| size.checked_add(4096))
        .ok_or_else(|| invalid("History contributor comparison allocation overflow"))?;
    let _grant = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    budget
        .charge(WorkUsage {
            reads: records,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let mut commits = BTreeMap::new();
    for commit in &source.commits {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        let _previous = commits.insert(commit.sha, commit);
    }
    let mut paths = BTreeMap::<&str, BTreeSet<Contributor<'_>>>::new();
    for hunk in &source.diff_hunks {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        let commit = commits
            .get(&hunk.commit_sha)
            .ok_or_else(|| invalid("History contributor hunk has no original commit"))?;
        let author = commit.author.trim();
        if author.is_empty() {
            return Err(invalid("History contributor author is empty"));
        }
        let name = commit
            .author_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let email = commit
            .author_email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let _inserted = paths
            .entry(hunk.file_path.as_ref())
            .or_default()
            .insert((author, name, email));
    }
    if output.entries.len() != paths.len() {
        return Err(invalid(
            "History file-contributor output has an incomplete path set",
        ));
    }
    for ((path, expected), observed) in paths.into_iter().zip(&output.entries) {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        if observed.source_repo_id != source.repo_id
            || observed.repo_relative_path.as_str() != path
            || observed.contributors.len() != expected.len()
        {
            return Err(invalid(
                "History file-contributor output differs from original paths",
            ));
        }
        for ((author, name, email), identity) in expected.into_iter().zip(&observed.contributors) {
            budget
                .checkpoint()
                .map_err(|error| history_work_error(budget, error))?;
            if (
                identity.canonical.as_str(),
                identity.name.as_deref(),
                identity.email.as_deref(),
            ) != (author, name, email)
            {
                return Err(invalid(
                    "History file-contributor identities differ from original commits",
                ));
            }
        }
    }
    Ok(())
}
