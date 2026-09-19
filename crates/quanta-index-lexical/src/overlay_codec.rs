//! The repo-metadata overlay families: which file each lives in, and its
//! snapshot and batch encodings.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::index_store::sidecar_corrupt;
use crate::metadata_normalize::{
    normalize_contributor_identity_entry, normalize_owner_identity, normalize_repo_meta_key,
    normalize_repo_topic_value,
};
use crate::{
    FileContributorShard, FileOwnershipShard, LEGACY_FULL_BUNDLE_PAYLOAD,
    LexicalRepoMetadataPayload, OverlaySnapshot, RepoCommitRecencyShard, RepoDescriptionShard,
    RepoMetaShard, RepoTopicShard,
};
use ciborium::Value as CborValue;
use quanta_index_contract::{
    FileContributorIdentityEntry, FileContributorIngestBatch, FileOwnershipIngestBatch,
    LqVisibility, RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoMetaIngestBatch,
    RepoTopicIngestBatch,
};
use quanta_index_core::CoreError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One overlay family, by the file it lives in.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum OverlayFamily {
    /// The `FullBundle` repo-metadata payload (fork, archived, visibility,
    /// contexts).
    RepoMetadata,
    /// Latest committer time per source repo.
    CommitRecency,
    /// `key:value` metadata per source repo.
    Meta,
    /// Topics per source repo.
    Topic,
    /// Description per source repo.
    Description,
    /// Owners per file.
    FileOwnership,
    /// Contributors per file.
    Contributor,
}

impl OverlayFamily {
    /// Every family, in manifest order.
    pub(crate) const ALL: [Self; 7] = [
        Self::RepoMetadata,
        Self::CommitRecency,
        Self::Meta,
        Self::Topic,
        Self::Description,
        Self::FileOwnership,
        Self::Contributor,
    ];

    /// The family's file name inside the generation directory.
    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::RepoMetadata => "repo-metadata.cbor",
            Self::CommitRecency => "repo-commit-recency.cbor",
            Self::Meta => "repo-meta.cbor",
            Self::Topic => "repo-topic.cbor",
            Self::Description => "repo-description.cbor",
            Self::FileOwnership => "file-ownership.cbor",
            Self::Contributor => "file-contributor.cbor",
        }
    }

    /// The family a manifest entry names, if any.
    pub(crate) fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.file_name() == name)
    }

    /// The family's path inside `generation_dir`.
    pub(crate) fn path(self, generation_dir: &Path) -> PathBuf {
        generation_dir.join(self.file_name())
    }
}

pub(crate) fn encode_repo_metadata_visibility(
    visibility: &LqVisibility,
) -> Result<CborValue, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(visibility, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo metadata encode visibility: {err}"))
    })?;
    ciborium::from_reader::<CborValue, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata visibility wire decode: {err}"
        ))
    })
}

pub(crate) fn decode_repo_metadata_bool(
    field_name: &str,
    value: &CborValue,
) -> Result<bool, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(&value, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` re-encode: {err}"
        ))
    })?;
    ciborium::from_reader::<bool, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` decode: {err}"
        ))
    })
}

pub(crate) fn decode_repo_metadata_visibility(
    field_name: &str,
    value: &CborValue,
) -> Result<LqVisibility, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(&value, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` re-encode: {err}"
        ))
    })?;
    ciborium::from_reader::<LqVisibility, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` decode: {err}"
        ))
    })
}

pub(crate) fn decode_repo_metadata_contexts(
    field_name: &str,
    value: &CborValue,
) -> Result<Vec<String>, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(&value, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` re-encode: {err}"
        ))
    })?;
    let contexts = ciborium::from_reader::<Vec<String>, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` decode: {err}"
        ))
    })?;
    if contexts.iter().any(String::is_empty) {
        return Err(CoreError::InvalidContract(
            "lexical: repo metadata field `contexts` must not contain empty names".to_string(),
        ));
    }
    Ok(contexts)
}

pub(crate) fn encode_repo_metadata_payload(
    metadata: &LexicalRepoMetadataPayload,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let wire = CborValue::Map(vec![
        (
            CborValue::Text("fork".to_string()),
            CborValue::Bool(metadata.fork),
        ),
        (
            CborValue::Text("archived".to_string()),
            CborValue::Bool(metadata.archived),
        ),
        (
            CborValue::Text("visibility".to_string()),
            encode_repo_metadata_visibility(&metadata.visibility)?,
        ),
        (
            CborValue::Text("contexts".to_string()),
            CborValue::Array(
                metadata
                    .contexts
                    .iter()
                    .cloned()
                    .map(CborValue::Text)
                    .collect(),
            ),
        ),
    ]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo metadata encode: {err}"))
    })?;
    Ok(payload)
}

pub(crate) fn decode_repo_metadata_payload(
    bytes: &[u8],
) -> Result<Option<LexicalRepoMetadataPayload>, CoreError> {
    if bytes.is_empty() || bytes == LEGACY_FULL_BUNDLE_PAYLOAD {
        return Ok(None);
    }
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo metadata payload decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo metadata payload decode: expected map".to_string(),
        ));
    };
    let mut fork: Option<bool> = None;
    let mut archived: Option<bool> = None;
    let mut visibility: Option<LqVisibility> = None;
    let mut contexts: Option<Vec<String>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo metadata payload decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "fork" => {
                if fork.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `fork`".to_string(),
                    ));
                }
                fork = Some(decode_repo_metadata_bool("fork", &value)?);
            }
            "archived" => {
                if archived.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `archived`"
                            .to_string(),
                    ));
                }
                archived = Some(decode_repo_metadata_bool("archived", &value)?);
            }
            "visibility" => {
                if visibility.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `visibility`"
                            .to_string(),
                    ));
                }
                visibility = Some(decode_repo_metadata_visibility("visibility", &value)?);
            }
            "contexts" => {
                if contexts.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `contexts`"
                            .to_string(),
                    ));
                }
                contexts = Some(decode_repo_metadata_contexts("contexts", &value)?);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo metadata payload decode: unknown field `{other}`"
                )));
            }
        }
    }
    Ok(Some(LexicalRepoMetadataPayload {
        fork: fork.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `fork`".to_string(),
            )
        })?,
        archived: archived.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `archived`".to_string(),
            )
        })?,
        visibility: visibility.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `visibility`".to_string(),
            )
        })?,
        contexts: contexts.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `contexts`".to_string(),
            )
        })?,
    }))
}

pub(crate) fn encode_repo_commit_recency_snapshot(
    shard: &RepoCommitRecencyShard,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let entries = shard
        .latest_committer_time_ms_by_repo_id
        .iter()
        .map(|(source_repo_id, latest_committer_time_ms)| {
            CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("latest_committer_time_ms".to_string()),
                    CborValue::Text(latest_committer_time_ms.to_string()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo commit recency encode: {err}"))
    })?;
    Ok(payload)
}

pub(crate) fn decode_repo_commit_recency_snapshot(
    bytes: &[u8],
) -> Result<RepoCommitRecencyShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo commit recency decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo commit recency decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo commit recency decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo commit recency decode: duplicate field `entries`"
                            .to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo commit recency decode: `entries` must be an array"
                            .to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo commit recency decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut latest_committer_time_ms_by_repo_id = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: repo commit recency decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo commit recency decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut latest_committer_time_ms: Option<u64> = None;
        for (key, value) in fields {
            let CborValue::Text(field_name) = key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo commit recency decode: entry field name must be text"
                        .to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo commit recency decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "latest_committer_time_ms" => {
                    let CborValue::Text(text) = value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo commit recency decode: `latest_committer_time_ms` must be text"
                                .to_string(),
                        ));
                    };
                    latest_committer_time_ms = Some(text.parse().map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "lexical: repo commit recency decode: `latest_committer_time_ms` must be a u64 decimal string: {err}"
                        ))
                    })?);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo commit recency decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo commit recency decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let latest_committer_time_ms = latest_committer_time_ms.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo commit recency decode: missing field `latest_committer_time_ms`"
                    .to_string(),
            )
        })?;
        let _prior =
            latest_committer_time_ms_by_repo_id.insert(source_repo_id, latest_committer_time_ms);
    }
    Ok(RepoCommitRecencyShard {
        latest_committer_time_ms_by_repo_id,
    })
}

/// The commit-recency snapshot one batch publishes, encoded.
pub(crate) fn encode_repo_commit_recency_batch(
    batch: &RepoCommitRecencyIngestBatch,
) -> Result<Vec<u8>, CoreError> {
    let mut latest_committer_time_ms_by_repo_id = BTreeMap::new();
    for entry in &batch.entries {
        let _prior = latest_committer_time_ms_by_repo_id.insert(
            entry.source_repo_id.as_str().to_string(),
            entry.latest_committer_time_ms,
        );
    }
    encode_repo_commit_recency_snapshot(&RepoCommitRecencyShard {
        latest_committer_time_ms_by_repo_id,
    })
}

pub(crate) fn encode_repo_meta_snapshot(shard: &RepoMetaShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, by_key) in &shard.meta_by_repo_id {
        for (key, value) in by_key {
            entries.push(CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("key".to_string()),
                    CborValue::Text(key.clone()),
                ),
                (
                    CborValue::Text("value".to_string()),
                    CborValue::Text(value.clone()),
                ),
            ]));
        }
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo meta encode: {err}")))?;
    Ok(payload)
}

pub(crate) fn decode_repo_meta_snapshot(bytes: &[u8]) -> Result<RepoMetaShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo meta decode: {err}")))?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo meta decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo meta decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo meta decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo meta decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo meta decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut meta_by_repo_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract("lexical: repo meta decode: missing field `entries`".to_string())
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo meta decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut key: Option<String> = None;
        let mut value: Option<String> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo meta decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo meta decode: `source_repo_id` must be text".to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "key" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo meta decode: `key` must be text".to_string(),
                        ));
                    };
                    key = Some(text);
                }
                "value" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo meta decode: `value` must be text".to_string(),
                        ));
                    };
                    value = Some(text);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo meta decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo meta decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let key = key.ok_or_else(|| {
            CoreError::InvalidContract("lexical: repo meta decode: missing field `key`".to_string())
        })?;
        let value = value.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo meta decode: missing field `value`".to_string(),
            )
        })?;
        let _prior = meta_by_repo_id
            .entry(source_repo_id)
            .or_default()
            .insert(key, value);
    }
    Ok(RepoMetaShard { meta_by_repo_id })
}

/// The repo-meta snapshot one batch publishes, encoded.
pub(crate) fn encode_repo_meta_batch(batch: &RepoMetaIngestBatch) -> Result<Vec<u8>, CoreError> {
    let mut meta_by_repo_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for entry in &batch.entries {
        let normalized_key = normalize_repo_meta_key(&entry.key).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo meta ingest entry key must not be empty".to_string(),
            )
        })?;
        let _prior = meta_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(normalized_key, entry.value.clone());
    }
    encode_repo_meta_snapshot(&RepoMetaShard { meta_by_repo_id })
}

pub(crate) fn encode_repo_topic_snapshot(shard: &RepoTopicShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, topics) in &shard.topics_by_repo_id {
        entries.push(CborValue::Map(vec![
            (
                CborValue::Text("source_repo_id".to_string()),
                CborValue::Text(source_repo_id.clone()),
            ),
            (
                CborValue::Text("topics".to_string()),
                CborValue::Array(
                    topics
                        .iter()
                        .cloned()
                        .map(CborValue::Text)
                        .collect::<Vec<_>>(),
                ),
            ),
        ]));
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo topic encode: {err}")))?;
    Ok(payload)
}

pub(crate) fn decode_repo_topic_snapshot(bytes: &[u8]) -> Result<RepoTopicShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo topic decode: {err}")))?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo topic decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo topic decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo topic decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo topic decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo topic decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut topics_by_repo_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: repo topic decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo topic decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut topics: Option<BTreeSet<String>> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo topic decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo topic decode: `source_repo_id` must be text".to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "topics" => {
                    let CborValue::Array(items) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo topic decode: `topics` must be an array".to_string(),
                        ));
                    };
                    let mut normalized: BTreeSet<String> = BTreeSet::new();
                    for item in items {
                        let CborValue::Text(text) = item else {
                            return Err(CoreError::InvalidContract(
                                "lexical: repo topic decode: topic must be text".to_string(),
                            ));
                        };
                        let topic = normalize_repo_topic_value(&text).ok_or_else(|| {
                            CoreError::InvalidContract(
                                "lexical: repo topic decode: topic must be non-empty".to_string(),
                            )
                        })?;
                        let _inserted = normalized.insert(topic);
                    }
                    topics = Some(normalized);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo topic decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo topic decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let topics = topics.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo topic decode: missing field `topics`".to_string(),
            )
        })?;
        let _prior = topics_by_repo_id.insert(source_repo_id, topics);
    }
    Ok(RepoTopicShard { topics_by_repo_id })
}

/// The repo-topic snapshot one batch publishes, encoded.
pub(crate) fn encode_repo_topic_batch(batch: &RepoTopicIngestBatch) -> Result<Vec<u8>, CoreError> {
    let mut topics_by_repo_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in &batch.entries {
        let topic = normalize_repo_topic_value(&entry.topic).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo topic ingest entry topic must not be empty".to_string(),
            )
        })?;
        let _inserted = topics_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(topic);
    }
    encode_repo_topic_snapshot(&RepoTopicShard { topics_by_repo_id })
}

/// Validate a producer-published repo description.
///
/// Unlike topics, the description is stored verbatim (case and internal
/// whitespace preserved) so regex matching at query time is faithful; only a
/// non-empty constraint is enforced. Returns the original string when it carries
/// a non-whitespace character, `None` otherwise.
pub(crate) fn validate_repo_description_value(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn encode_repo_description_snapshot(
    shard: &RepoDescriptionShard,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, description) in &shard.descriptions_by_repo_id {
        entries.push(CborValue::Map(vec![
            (
                CborValue::Text("source_repo_id".to_string()),
                CborValue::Text(source_repo_id.clone()),
            ),
            (
                CborValue::Text("description".to_string()),
                CborValue::Text(description.clone()),
            ),
        ]));
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo description encode: {err}"))
    })?;
    Ok(payload)
}

pub(crate) fn decode_repo_description_snapshot(
    bytes: &[u8],
) -> Result<RepoDescriptionShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo description decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo description decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo description decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo description decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo description decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo description decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut descriptions_by_repo_id: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: repo description decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo description decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut description: Option<String> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo description decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    if source_repo_id.is_some() {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: duplicate entry field `source_repo_id`"
                                .to_string(),
                        ));
                    }
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "description" => {
                    if description.is_some() {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: duplicate entry field `description`"
                                .to_string(),
                        ));
                    }
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: `description` must be text"
                                .to_string(),
                        ));
                    };
                    let value = validate_repo_description_value(&text).ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: repo description decode: description must be non-empty"
                                .to_string(),
                        )
                    })?;
                    description = Some(value);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo description decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo description decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let description = description.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo description decode: missing field `description`".to_string(),
            )
        })?;
        // A well-formed snapshot (written from a BTreeMap) has one entry per
        // repo; a duplicate means a corrupted or foreign file. Fail closed.
        if let Some(prior) = descriptions_by_repo_id.insert(source_repo_id.clone(), description) {
            return Err(CoreError::InvalidContract(format!(
                "lexical: repo description decode: duplicate entry for source_repo_id `{source_repo_id}` (prior `{prior}`)"
            )));
        }
    }
    Ok(RepoDescriptionShard {
        descriptions_by_repo_id,
    })
}

/// The repo-description snapshot one batch publishes, encoded.
pub(crate) fn encode_repo_description_batch(
    batch: &RepoDescriptionIngestBatch,
) -> Result<Vec<u8>, CoreError> {
    let mut descriptions_by_repo_id: BTreeMap<String, String> = BTreeMap::new();
    for entry in &batch.entries {
        let description = validate_repo_description_value(&entry.description).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo description ingest entry description must not be empty".to_string(),
            )
        })?;
        // The description is a single scalar per repo: a batch carrying two
        // entries for the same source repo is a malformed/conflicting authority
        // input. Fail closed rather than silently last-wins — never let a buggy
        // producer batch pick a description non-deterministically.
        if let Some(prior) =
            descriptions_by_repo_id.insert(entry.source_repo_id.as_str().to_string(), description)
        {
            return Err(CoreError::InvalidContract(format!(
                "lexical: repo description ingest carries conflicting entries for source_repo_id `{}` (prior `{prior}`); one description per repo per batch",
                entry.source_repo_id.as_str()
            )));
        }
    }
    encode_repo_description_snapshot(&RepoDescriptionShard {
        descriptions_by_repo_id,
    })
}

pub(crate) fn encode_file_ownership_snapshot(
    shard: &FileOwnershipShard,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, by_path) in &shard.owners_by_repo_id {
        for (repo_relative_path, owners) in by_path {
            entries.push(CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("repo_relative_path".to_string()),
                    CborValue::Text(repo_relative_path.clone()),
                ),
                (
                    CborValue::Text("owners".to_string()),
                    CborValue::Array(
                        owners
                            .iter()
                            .cloned()
                            .map(CborValue::Text)
                            .collect::<Vec<_>>(),
                    ),
                ),
            ]));
        }
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file ownership encode: {err}"))
    })?;
    Ok(payload)
}

pub(crate) fn decode_file_ownership_snapshot(
    bytes: &[u8],
) -> Result<FileOwnershipShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file ownership decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: file ownership decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: file ownership decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: file ownership decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: file ownership decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: file ownership decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut owners_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: file ownership decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: file ownership decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut repo_relative_path: Option<String> = None;
        let mut owners: Option<BTreeSet<String>> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: file ownership decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file ownership decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "repo_relative_path" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file ownership decode: `repo_relative_path` must be text"
                                .to_string(),
                        ));
                    };
                    repo_relative_path = Some(text);
                }
                "owners" => {
                    let CborValue::Array(items) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file ownership decode: `owners` must be an array".to_string(),
                        ));
                    };
                    let mut normalized: BTreeSet<String> = BTreeSet::new();
                    for item in items {
                        let CborValue::Text(text) = item else {
                            return Err(CoreError::InvalidContract(
                                "lexical: file ownership decode: owner must be text".to_string(),
                            ));
                        };
                        let owner = normalize_owner_identity(&text).ok_or_else(|| {
                            CoreError::InvalidContract(
                                "lexical: file ownership decode: owner must be non-empty"
                                    .to_string(),
                            )
                        })?;
                        let _inserted = normalized.insert(owner);
                    }
                    owners = Some(normalized);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: file ownership decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file ownership decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let repo_relative_path = repo_relative_path.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file ownership decode: missing field `repo_relative_path`".to_string(),
            )
        })?;
        let owners = owners.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file ownership decode: missing field `owners`".to_string(),
            )
        })?;
        let _prior = owners_by_repo_id
            .entry(source_repo_id)
            .or_default()
            .insert(repo_relative_path, owners);
    }
    Ok(FileOwnershipShard { owners_by_repo_id })
}

/// The file-ownership snapshot one batch publishes, encoded.
pub(crate) fn encode_file_ownership_batch(
    batch: &FileOwnershipIngestBatch,
) -> Result<Vec<u8>, CoreError> {
    let mut owners_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for entry in &batch.entries {
        let repo_relative_path = entry.repo_relative_path.as_str().to_string();
        let owners = entry
            .owners
            .iter()
            .map(|owner| {
                normalize_owner_identity(owner).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: file ownership ingest owner must not be empty".to_string(),
                    )
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let _prior = owners_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(repo_relative_path, owners);
    }
    encode_file_ownership_snapshot(&FileOwnershipShard { owners_by_repo_id })
}

pub(crate) fn encode_file_contributor_snapshot(
    shard: &FileContributorShard,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, by_path) in &shard.contributors_by_repo_id {
        for (repo_relative_path, contributors) in by_path {
            entries.push(CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("repo_relative_path".to_string()),
                    CborValue::Text(repo_relative_path.clone()),
                ),
                (
                    CborValue::Text("contributors".to_string()),
                    CborValue::Array(
                        contributors
                            .iter()
                            .cloned()
                            .map(|contributor| {
                                let mut fields = vec![(
                                    CborValue::Text("canonical".to_string()),
                                    CborValue::Text(contributor.canonical),
                                )];
                                if let Some(name) = contributor.name {
                                    fields.push((
                                        CborValue::Text("name".to_string()),
                                        CborValue::Text(name),
                                    ));
                                }
                                if let Some(email) = contributor.email {
                                    fields.push((
                                        CborValue::Text("email".to_string()),
                                        CborValue::Text(email),
                                    ));
                                }
                                CborValue::Map(fields)
                            })
                            .collect::<Vec<_>>(),
                    ),
                ),
            ]));
        }
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file contributor encode: {err}"))
    })?;
    Ok(payload)
}

pub(crate) fn decode_file_contributor_snapshot(
    bytes: &[u8],
) -> Result<FileContributorShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file contributor decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: file contributor decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: file contributor decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: file contributor decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: file contributor decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: file contributor decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut contributors_by_repo_id: BTreeMap<
        String,
        BTreeMap<String, BTreeSet<FileContributorIdentityEntry>>,
    > = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: file contributor decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: file contributor decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut repo_relative_path: Option<String> = None;
        let mut contributors: Option<BTreeSet<FileContributorIdentityEntry>> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: file contributor decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file contributor decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "repo_relative_path" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file contributor decode: `repo_relative_path` must be text"
                                .to_string(),
                        ));
                    };
                    repo_relative_path = Some(text);
                }
                "contributors" => {
                    let CborValue::Array(items) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file contributor decode: `contributors` must be an array"
                                .to_string(),
                        ));
                    };
                    let mut normalized: BTreeSet<FileContributorIdentityEntry> = BTreeSet::new();
                    for item in items {
                        let contributor = decode_file_contributor_identity_entry(&item)?;
                        let _inserted = normalized.insert(contributor);
                    }
                    contributors = Some(normalized);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: file contributor decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file contributor decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let repo_relative_path = repo_relative_path.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file contributor decode: missing field `repo_relative_path`".to_string(),
            )
        })?;
        let contributors = contributors.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file contributor decode: missing field `contributors`".to_string(),
            )
        })?;
        let _prior = contributors_by_repo_id
            .entry(source_repo_id)
            .or_default()
            .insert(repo_relative_path, contributors);
    }
    Ok(FileContributorShard {
        contributors_by_repo_id,
    })
}

/// The file-contributor snapshot one batch publishes, encoded.
pub(crate) fn encode_file_contributor_batch(
    batch: &FileContributorIngestBatch,
) -> Result<Vec<u8>, CoreError> {
    let mut contributors_by_repo_id: BTreeMap<
        String,
        BTreeMap<String, BTreeSet<FileContributorIdentityEntry>>,
    > = BTreeMap::new();
    for entry in &batch.entries {
        let repo_relative_path = entry.repo_relative_path.as_str().to_string();
        let contributors = entry
            .contributors
            .iter()
            .map(|contributor| {
                normalize_contributor_identity_entry(contributor).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: file contributor ingest contributor must not be empty"
                            .to_string(),
                    )
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let _prior = contributors_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(repo_relative_path, contributors);
    }
    encode_file_contributor_snapshot(&FileContributorShard {
        contributors_by_repo_id,
    })
}

/// Decode one overlay family's proved bytes; a decode failure names the
/// file and is a corrupt sealed generation, never a partial authority.
pub(crate) fn decode_overlay(
    family: OverlayFamily,
    bytes: &[u8],
    generation_dir: &Path,
) -> Result<OverlaySnapshot, CoreError> {
    let decoded = match family {
        OverlayFamily::RepoMetadata => decode_repo_metadata_payload(bytes).and_then(|payload| {
            payload.map(OverlaySnapshot::RepoMetadata).ok_or_else(|| {
                CoreError::InvalidContract("repo metadata snapshot carries no payload".to_string())
            })
        }),
        OverlayFamily::CommitRecency => {
            decode_repo_commit_recency_snapshot(bytes).map(OverlaySnapshot::CommitRecency)
        }
        OverlayFamily::Meta => decode_repo_meta_snapshot(bytes).map(OverlaySnapshot::Meta),
        OverlayFamily::Topic => decode_repo_topic_snapshot(bytes).map(OverlaySnapshot::Topic),
        OverlayFamily::Description => {
            decode_repo_description_snapshot(bytes).map(OverlaySnapshot::Description)
        }
        OverlayFamily::FileOwnership => {
            decode_file_ownership_snapshot(bytes).map(OverlaySnapshot::FileOwnership)
        }
        OverlayFamily::Contributor => {
            decode_file_contributor_snapshot(bytes).map(OverlaySnapshot::Contributor)
        }
    };
    decoded.map_err(|err| match err {
        CoreError::InvalidContract(message) => {
            sidecar_corrupt(generation_dir, family.file_name(), &message)
        }
        other @ (CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => other,
    })
}

// Fail-closed CBOR decode: every wildcard arm below rejects any value that is
// not the explicitly admitted shape (Text/Null per field, Text/Map per entry).
// `wildcard_enum_match_arm` is expected at the function level because the lint is
// emitted against the arm *pattern*, which an arm-local attribute does not cover.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "fail-closed decode: any CBOR variant outside the admitted shapes is a contract violation, caught by the wildcard arms"
)]
pub(crate) fn decode_file_contributor_identity_entry(
    value: &CborValue,
) -> Result<FileContributorIdentityEntry, CoreError> {
    match value {
        CborValue::Text(text) => {
            normalize_contributor_identity_entry(&FileContributorIdentityEntry {
                canonical: text.clone(),
                name: None,
                email: None,
            })
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "lexical: file contributor decode: contributor must be non-empty".to_string(),
                )
            })
        }
        CborValue::Map(fields) => {
            let mut canonical: Option<String> = None;
            let mut name: Option<Option<String>> = None;
            let mut email: Option<Option<String>> = None;
            for (field_key, field_value) in fields {
                let CborValue::Text(field_name) = field_key else {
                    return Err(CoreError::InvalidContract(
                        "lexical: file contributor decode: contributor field name must be text"
                            .to_string(),
                    ));
                };
                match field_name.as_str() {
                    "canonical" => {
                        let CborValue::Text(text) = field_value else {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor `canonical` must be text"
                                    .to_string(),
                            ));
                        };
                        canonical = Some(text.clone());
                    }
                    "name" => match field_value {
                        CborValue::Text(text) => name = Some(Some(text.clone())),
                        CborValue::Null => name = Some(None),
                        _ => {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor `name` must be text or null"
                                    .to_string(),
                            ));
                        }
                    },
                    "email" => match field_value {
                        CborValue::Text(text) => email = Some(Some(text.clone())),
                        CborValue::Null => email = Some(None),
                        _ => {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor `email` must be text or null"
                                    .to_string(),
                            ));
                        }
                    },
                    other => {
                        return Err(CoreError::InvalidContract(format!(
                            "lexical: file contributor decode: unknown contributor field `{other}`"
                        )));
                    }
                }
            }
            normalize_contributor_identity_entry(&FileContributorIdentityEntry {
                canonical: canonical.ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: file contributor decode: contributor missing field `canonical`"
                            .to_string(),
                    )
                })?,
                name: name.unwrap_or(None),
                email: email.unwrap_or(None),
            })
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "lexical: file contributor decode: contributor must be non-empty".to_string(),
                )
            })
        }
        _ => Err(CoreError::InvalidContract(
            "lexical: file contributor decode: contributor must be text or map".to_string(),
        )),
    }
}
