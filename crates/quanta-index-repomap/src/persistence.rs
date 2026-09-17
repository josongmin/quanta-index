//! Durable `RepoMap` snapshots and activations (QI-BB-008).
//!
//! Every file is written whole or not at all: the bytes go to a temporary
//! in the same directory, are synced, renamed over the final name, and the
//! directory is synced, so a crash at any point leaves either the previous
//! file or the new one. A snapshot file carries a digest of its canonical
//! content and is refused when the bytes no longer match. A file this store
//! cannot trust is moved to `quarantine/` with its reason and the rest of
//! the store opens; one bad file never fails the whole state root. Snapshot
//! files from before the digest envelope are read once and rewritten under
//! it.

use core::fmt;
use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    CoreError, QUARANTINE_TARGET_NOT_QUARANTINED_CODE, QuarantineDiscardOutcomeV1,
    QuarantinedRepoMapFileV1, RepoMapOpenReportV1,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};

use crate::model::RepoMapSnapshot;

const SNAPSHOT_FILE_FORMAT_VERSION: u32 = 2;
const TEMPORARY_PREFIX: &str = ".tmp-";
const JSON_EXTENSION: &str = "json";

#[cfg(test)]
const CRASH_BOUNDARY_ENV: &str = "QUANTA_INDEX_REPOMAP_CRASH_BOUNDARY";
#[cfg(test)]
const CRASH_EXIT_CODE: i32 = 87;
/// The write protocol's crash points, named for the crash matrix.
const AFTER_TEMPORARY_SYNC: &str = "after-temporary-sync";
const AFTER_RENAME: &str = "after-rename";

/// Stop the process dead at a named point of the write protocol when the
/// crash matrix asks for it; a subprocess-only hook.
#[cfg(test)]
#[expect(
    clippy::exit,
    reason = "the subprocess-only crash matrix must terminate without unwinding, exactly as a crash would"
)]
fn exit_at_crash_boundary(boundary: &str) {
    if std::env::var(CRASH_BOUNDARY_ENV).is_ok_and(|configured| configured == boundary) {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

#[cfg(not(test))]
const fn exit_at_crash_boundary(_boundary: &str) {}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private persistence module still needs sibling-module visibility"
)]
#[derive(Clone, Debug)]
pub(crate) struct RepoMapSnapshotPersistence {
    snapshots: PathBuf,
    activations: PathBuf,
    quarantine: PathBuf,
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private persistence module still needs sibling-module visibility"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoMapActivationRecordV1 {
    pub(crate) repo_id: String,
    pub(crate) revision_id: String,
    pub(crate) manifest_generation: u64,
}

/// Everything the store found on disk at open, plus what it did about it.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private persistence module still needs sibling-module visibility"
)]
pub(crate) struct LoadedRepoMapFilesV1 {
    pub(crate) snapshots: Vec<RepoMapSnapshot>,
    pub(crate) activations: Vec<RepoMapActivationRecordV1>,
    pub(crate) report: RepoMapOpenReportV1,
}

const REPOMAP_ACTIVATION_RECORD_V1_FIELDS: &[&str] =
    &["repo_id", "revision_id", "manifest_generation"];

impl Serialize for RepoMapActivationRecordV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapActivationRecordV1", 3)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.end()
    }
}

struct RepoMapActivationRecordV1Visitor;

impl<'de> Visitor<'de> for RepoMapActivationRecordV1Visitor {
    type Value = RepoMapActivationRecordV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapActivationRecordV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<String> = None;
        let mut revision_id: Option<String> = None;
        let mut manifest_generation: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_ACTIVATION_RECORD_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapActivationRecordV1 {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapActivationRecordV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapActivationRecordV1",
            REPOMAP_ACTIVATION_RECORD_V1_FIELDS,
            RepoMapActivationRecordV1Visitor,
        )
    }
}

/// The on-disk snapshot envelope: a digest of the canonical snapshot bytes
/// beside the snapshot, so a rewritten or bit-rotted file is refused.
struct RepoMapSnapshotFileV2 {
    format_version: u32,
    sha256: String,
    snapshot: RepoMapSnapshot,
}

const REPOMAP_SNAPSHOT_FILE_V2_FIELDS: &[&str] = &["format_version", "sha256", "snapshot"];

impl Serialize for RepoMapSnapshotFileV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSnapshotFileV2", 3)?;
        state.serialize_field("format_version", &self.format_version)?;
        state.serialize_field("sha256", &self.sha256)?;
        state.serialize_field("snapshot", &self.snapshot)?;
        state.end()
    }
}

struct RepoMapSnapshotFileV2Visitor;

impl<'de> Visitor<'de> for RepoMapSnapshotFileV2Visitor {
    type Value = RepoMapSnapshotFileV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSnapshotFileV2 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut format_version: Option<u32> = None;
        let mut sha256: Option<String> = None;
        let mut snapshot: Option<RepoMapSnapshot> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "format_version" => {
                    if format_version.is_some() {
                        return Err(de::Error::duplicate_field("format_version"));
                    }
                    format_version = Some(map.next_value()?);
                }
                "sha256" => {
                    if sha256.is_some() {
                        return Err(de::Error::duplicate_field("sha256"));
                    }
                    sha256 = Some(map.next_value()?);
                }
                "snapshot" => {
                    if snapshot.is_some() {
                        return Err(de::Error::duplicate_field("snapshot"));
                    }
                    snapshot = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_SNAPSHOT_FILE_V2_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapSnapshotFileV2 {
            format_version: format_version
                .ok_or_else(|| de::Error::missing_field("format_version"))?,
            sha256: sha256.ok_or_else(|| de::Error::missing_field("sha256"))?,
            snapshot: snapshot.ok_or_else(|| de::Error::missing_field("snapshot"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapSnapshotFileV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSnapshotFileV2",
            REPOMAP_SNAPSHOT_FILE_V2_FIELDS,
            RepoMapSnapshotFileV2Visitor,
        )
    }
}

/// Lower-case hex SHA-256 of the snapshot's canonical (compact) JSON.
fn snapshot_digest_hex(snapshot: &RepoMapSnapshot) -> Result<String, CoreError> {
    let canonical = serde_json::to_vec(snapshot).map_err(|err| {
        CoreError::Storage(format!(
            "repomap persistence failed to encode canonical snapshot bytes: {err}"
        ))
    })?;
    let digest: [u8; 32] = Sha256::digest(&canonical).into();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(lower_hex_char(byte >> 4));
        hex.push(lower_hex_char(byte & 0x0F));
    }
    Ok(hex)
}

fn lower_hex_char(nibble: u8) -> char {
    const HEX_DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    HEX_DIGITS
        .get(usize::from(nibble))
        .copied()
        .map_or('0', std::convert::identity)
}

/// Suffix of the file that records why its sibling was quarantined.
const QUARANTINE_REASON_SUFFIX: &str = ".reason";
/// What a quarantined file is listed under when no reason sits beside it:
/// it was moved aside by a build that did not yet record reasons.
const QUARANTINE_REASON_NOT_RECORDED: &str =
    "reason not recorded: quarantined before reasons were persisted";

fn quarantine_reason_path(quarantine: &Path, file_name: &str) -> PathBuf {
    quarantine.join(format!("{file_name}{QUARANTINE_REASON_SUFFIX}"))
}

fn storage_error(action: &str, path: &Path, err: &dyn fmt::Display) -> CoreError {
    CoreError::Storage(format!(
        "repomap persistence failed to {action} {}: {err}",
        path.display()
    ))
}

/// Write `bytes` to `dir/file_name` whole or not at all.
///
/// Temporary in the same directory, file sync, rename over the final name,
/// directory sync: a crash between any two steps leaves the previous file
/// (and at most a temporary the next open sweeps) or the new one.
fn write_atomic(dir: &Path, file_name: &str, bytes: &[u8]) -> Result<(), CoreError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temporary = dir.join(format!(
        "{TEMPORARY_PREFIX}{file_name}.{}.{nanos}",
        std::process::id()
    ));
    let final_path = dir.join(file_name);
    let written = write_atomic_steps(dir, &temporary, &final_path, bytes);
    if written.is_err() && temporary.exists() {
        // The rename never happened; the temporary is the only trace and
        // must not survive as a stray. Its removal failing cannot outrank
        // the write failure being reported.
        let _removed: Result<(), std::io::Error> = fs::remove_file(&temporary);
    }
    written
}

fn write_atomic_steps(
    dir: &Path,
    temporary: &Path,
    final_path: &Path,
    bytes: &[u8],
) -> Result<(), CoreError> {
    let mut file =
        File::create(temporary).map_err(|err| storage_error("create", temporary, &err))?;
    file.write_all(bytes)
        .map_err(|err| storage_error("write", temporary, &err))?;
    file.sync_all()
        .map_err(|err| storage_error("sync", temporary, &err))?;
    drop(file);
    exit_at_crash_boundary(AFTER_TEMPORARY_SYNC);
    fs::rename(temporary, final_path)
        .map_err(|err| storage_error("rename into place", temporary, &err))?;
    exit_at_crash_boundary(AFTER_RENAME);
    File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| storage_error("sync directory", dir, &err))?;
    Ok(())
}

impl RepoMapSnapshotPersistence {
    pub(crate) fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        let snapshots = root.join("snapshots");
        let activations = root.join("activations");
        let quarantine = root.join("quarantine");
        for dir in [&snapshots, &activations, &quarantine] {
            fs::create_dir_all(dir).map_err(|err| storage_error("create dir", dir, &err))?;
        }
        Ok(Self {
            snapshots,
            activations,
            quarantine,
        })
    }

    /// Read every snapshot and activation the store holds.
    ///
    /// Sweeps temporaries a crash left behind, rewrites pre-envelope
    /// snapshots under the digest envelope, and quarantines every file
    /// that does not decode or whose digest does not match — each with its
    /// reason in the report, none of them failing the open.
    pub(crate) fn load(&self) -> Result<LoadedRepoMapFilesV1, CoreError> {
        let stale_temporaries_removed = self
            .sweep_temporaries(&self.snapshots)?
            .saturating_add(self.sweep_temporaries(&self.activations)?);
        let mut report = RepoMapOpenReportV1 {
            stale_temporaries_removed,
            ..RepoMapOpenReportV1::default()
        };
        let mut snapshots = Vec::new();
        for path in self.list_json_files(&self.snapshots)? {
            match self.read_snapshot_file(&path) {
                Ok((snapshot, migrated)) => {
                    if migrated {
                        report.snapshots_migrated = report.snapshots_migrated.saturating_add(1);
                    }
                    report.snapshots_loaded = report.snapshots_loaded.saturating_add(1);
                    snapshots.push(snapshot);
                }
                Err(reason) => self.quarantine(&path, reason, &mut report)?,
            }
        }
        let mut activations = Vec::new();
        for path in self.list_json_files(&self.activations)? {
            match Self::read_activation_file(&path) {
                Ok(activation) => {
                    report.activations_loaded = report.activations_loaded.saturating_add(1);
                    activations.push(activation);
                }
                Err(reason) => self.quarantine(&path, reason, &mut report)?,
            }
        }
        Ok(LoadedRepoMapFilesV1 {
            snapshots,
            activations,
            report,
        })
    }

    /// Decode one snapshot file; `true` when it was a pre-envelope file that
    /// has now been rewritten under the envelope. The error is the reason a
    /// file is quarantined, never a store failure.
    fn read_snapshot_file(&self, path: &Path) -> Result<(RepoMapSnapshot, bool), String> {
        let bytes = fs::read(path).map_err(|err| format!("read: {err}"))?;
        match serde_json::from_slice::<RepoMapSnapshotFileV2>(&bytes) {
            Ok(file) => {
                if file.format_version != SNAPSHOT_FILE_FORMAT_VERSION {
                    return Err(format!(
                        "snapshot file format {} is not {SNAPSHOT_FILE_FORMAT_VERSION}",
                        file.format_version
                    ));
                }
                let expected =
                    snapshot_digest_hex(&file.snapshot).map_err(|err| err.to_string())?;
                if expected != file.sha256 {
                    return Err(format!(
                        "snapshot digest {} does not match its content digest {expected}",
                        file.sha256
                    ));
                }
                if self.snapshots.join(snapshot_file_name(&file.snapshot)) != path {
                    return Err(format!(
                        "snapshot names repo={} revision={} generation={} but sits under another file name",
                        file.snapshot.repo_id.as_str(),
                        file.snapshot.revision_id.as_str(),
                        file.snapshot.manifest_generation.get()
                    ));
                }
                Ok((file.snapshot, false))
            }
            Err(envelope_err) => {
                // Before the envelope, a file was the bare snapshot; it is
                // read once and rewritten under the envelope so the next
                // open verifies it like every other file.
                let legacy = serde_json::from_slice::<RepoMapSnapshot>(&bytes).map_err(|legacy_err| {
                    format!("decode: not a snapshot envelope ({envelope_err}) nor a legacy snapshot ({legacy_err})")
                })?;
                if self.snapshots.join(snapshot_file_name(&legacy)) != path {
                    return Err(
                        "legacy snapshot sits under a file name that does not match its identity"
                            .to_string(),
                    );
                }
                self.persist_snapshot(&legacy)
                    .map_err(|err| format!("rewrite under the digest envelope: {err}"))?;
                Ok((legacy, true))
            }
        }
    }

    fn read_activation_file(path: &Path) -> Result<RepoMapActivationRecordV1, String> {
        let bytes = fs::read(path).map_err(|err| format!("read: {err}"))?;
        let record = serde_json::from_slice::<RepoMapActivationRecordV1>(&bytes)
            .map_err(|err| format!("decode: {err}"))?;
        let expected = activation_file_name(
            &RepoId::new(&record.repo_id),
            &RevisionId::new(&record.revision_id),
        );
        if path.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
            return Err(format!(
                "activation names repo={} revision={} but sits under another file name",
                record.repo_id, record.revision_id
            ));
        }
        Ok(record)
    }

    fn quarantine(
        &self,
        path: &Path,
        reason: String,
        report: &mut RepoMapOpenReportV1,
    ) -> Result<(), CoreError> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                CoreError::Storage(format!(
                    "repomap persistence cannot quarantine a file without a name: {}",
                    path.display()
                ))
            })?;
        let destination = self.quarantine.join(&file_name);
        fs::rename(path, &destination).map_err(|err| storage_error("quarantine", path, &err))?;
        // The reason sits beside the file so a later process can still say
        // why it was set aside (QI-BB-026).
        let reason_path = quarantine_reason_path(&self.quarantine, &file_name);
        fs::write(&reason_path, reason.as_bytes())
            .map_err(|err| storage_error("record quarantine reason", &reason_path, &err))?;
        File::open(&self.quarantine)
            .and_then(|directory| directory.sync_all())
            .map_err(|err| storage_error("sync directory", &self.quarantine, &err))?;
        report
            .quarantined
            .push(QuarantinedRepoMapFileV1 { file_name, reason });
        Ok(())
    }

    /// Every file in the quarantine directory right now, with the reason
    /// recorded beside it.
    pub(crate) fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.quarantine)
            .map_err(|err| storage_error("list dir", &self.quarantine, &err))?
        {
            let entry =
                entry.map_err(|err| storage_error("read dir entry in", &self.quarantine, &err))?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if file_name.ends_with(QUARANTINE_REASON_SUFFIX) {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|err| storage_error("inspect", &entry.path(), &err))?
                .is_file()
            {
                continue;
            }
            let reason = self.quarantine_reason(&file_name)?;
            out.push(QuarantinedRepoMapFileV1 { file_name, reason });
        }
        out.sort_by(|left, right| left.file_name.cmp(&right.file_name));
        Ok(out)
    }

    /// The reason recorded beside a quarantined file.
    ///
    /// A file quarantined before reasons were recorded has none on disk;
    /// it is listed under a fixed sentence saying so, and a discard must
    /// name that sentence back, exactly as with any other reason.
    fn quarantine_reason(&self, file_name: &str) -> Result<String, CoreError> {
        let reason_path = quarantine_reason_path(&self.quarantine, file_name);
        match fs::read_to_string(&reason_path) {
            Ok(reason) => Ok(reason),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(QUARANTINE_REASON_NOT_RECORDED.to_string())
            }
            Err(err) => Err(storage_error("read quarantine reason", &reason_path, &err)),
        }
    }

    /// Remove one quarantined file and its recorded reason.
    ///
    /// `entry` names the file as [`Self::quarantined_files`] listed it: a
    /// single path segment naming a regular file in the quarantine
    /// directory, under the reason recorded beside it right now. A name
    /// that is already gone is `Absent`; anything else — a nested name, a
    /// directory, a reason the record does not carry — is refused typed,
    /// so a listing that went stale removes nothing.
    pub(crate) fn discard_quarantined_file(
        &self,
        entry: &QuarantinedRepoMapFileV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        let file_name = entry.file_name.as_str();
        let refuse = |why: String| CoreError::Typed {
            code: QUARANTINE_TARGET_NOT_QUARANTINED_CODE.to_string(),
            message: format!("repomap: refusing to discard quarantined `{file_name}`: {why}"),
        };
        if file_name.is_empty()
            || file_name == "."
            || file_name == ".."
            || file_name.contains('/')
            || file_name.ends_with(QUARANTINE_REASON_SUFFIX)
        {
            return Err(refuse(
                "the name is not a quarantined file's name".to_string(),
            ));
        }
        let path = self.quarantine.join(file_name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(QuarantineDiscardOutcomeV1::Absent);
            }
            Err(err) => return Err(storage_error("inspect quarantined", &path, &err)),
        };
        if !metadata.is_file() {
            return Err(refuse(
                "it is not a regular file in the quarantine directory".to_string(),
            ));
        }
        let recorded = self.quarantine_reason(file_name)?;
        if recorded != entry.reason {
            return Err(refuse(format!(
                "it is recorded as quarantined for `{recorded}` now, not `{}` as listed; list again",
                entry.reason
            )));
        }
        let bytes = metadata.len();
        fs::remove_file(&path).map_err(|err| storage_error("discard quarantined", &path, &err))?;
        let reason_path = quarantine_reason_path(&self.quarantine, file_name);
        match fs::remove_file(&reason_path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(storage_error(
                    "discard quarantine reason",
                    &reason_path,
                    &err,
                ));
            }
        }
        File::open(&self.quarantine)
            .and_then(|directory| directory.sync_all())
            .map_err(|err| storage_error("sync directory", &self.quarantine, &err))?;
        Ok(QuarantineDiscardOutcomeV1::Discarded { bytes })
    }

    /// Remove temporaries a crashed write left in `dir`; their content was
    /// never named, so nothing is lost by removing them.
    fn sweep_temporaries(&self, dir: &Path) -> Result<u64, CoreError> {
        let mut removed = 0_u64;
        for entry in fs::read_dir(dir).map_err(|err| storage_error("list dir", dir, &err))? {
            let entry = entry.map_err(|err| storage_error("read dir entry in", dir, &err))?;
            let is_temporary = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(TEMPORARY_PREFIX));
            if !is_temporary {
                continue;
            }
            let path = entry.path();
            fs::remove_file(&path).map_err(|err| storage_error("remove temporary", &path, &err))?;
            removed = removed.saturating_add(1);
        }
        if removed > 0 {
            File::open(dir)
                .and_then(|directory| directory.sync_all())
                .map_err(|err| storage_error("sync directory", dir, &err))?;
        }
        Ok(removed)
    }

    pub(crate) fn persist_snapshot(&self, snapshot: &RepoMapSnapshot) -> Result<(), CoreError> {
        let file_name = snapshot_file_name(snapshot);
        let file = RepoMapSnapshotFileV2 {
            format_version: SNAPSHOT_FILE_FORMAT_VERSION,
            sha256: snapshot_digest_hex(snapshot)?,
            snapshot: snapshot.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to encode snapshot {file_name}: {err}"
            ))
        })?;
        write_atomic(&self.snapshots, &file_name, &bytes)
    }

    pub(crate) fn persist_activation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        manifest_generation: u64,
    ) -> Result<(), CoreError> {
        let file_name = activation_file_name(repo_id, revision_id);
        let record = RepoMapActivationRecordV1 {
            repo_id: repo_id.as_str().to_string(),
            revision_id: revision_id.as_str().to_string(),
            manifest_generation,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to encode activation {file_name}: {err}"
            ))
        })?;
        write_atomic(&self.activations, &file_name, &bytes)
    }

    /// Remove one generation's snapshot file; absent is not an error, since
    /// retention may run again over the same generations.
    pub(crate) fn remove_snapshot(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        manifest_generation: ManifestGeneration,
    ) -> Result<(), CoreError> {
        let path = self.snapshots.join(snapshot_file_name_for(
            repo_id,
            revision_id,
            manifest_generation,
        ));
        match fs::remove_file(&path) {
            Ok(()) => File::open(&self.snapshots)
                .and_then(|directory| directory.sync_all())
                .map_err(|err| storage_error("sync directory", &self.snapshots, &err)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(storage_error("remove snapshot", &path, &err)),
        }
    }

    fn list_json_files(&self, dir: &Path) -> Result<Vec<PathBuf>, CoreError> {
        let mut paths = Vec::new();
        let entries = fs::read_dir(dir).map_err(|err| storage_error("list dir", dir, &err))?;
        for entry in entries {
            let entry = entry.map_err(|err| storage_error("read dir entry in", dir, &err))?;
            let file_type = entry
                .file_type()
                .map_err(|err| storage_error("inspect dir entry in", dir, &err))?;
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == JSON_EXTENSION) {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }
}

fn snapshot_file_name(snapshot: &RepoMapSnapshot) -> String {
    snapshot_file_name_for(
        &snapshot.repo_id,
        &snapshot.revision_id,
        snapshot.manifest_generation,
    )
}

fn snapshot_file_name_for(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    manifest_generation: ManifestGeneration,
) -> String {
    format!(
        "{}--{}--g{}.{JSON_EXTENSION}",
        encode_component(repo_id.as_str()),
        encode_component(revision_id.as_str()),
        manifest_generation.get()
    )
}

fn activation_file_name(repo_id: &RepoId, revision_id: &RevisionId) -> String {
    format!(
        "{}--{}.{JSON_EXTENSION}",
        encode_component(repo_id.as_str()),
        encode_component(revision_id.as_str())
    )
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.' {
            encoded.push(char::from(byte));
            continue;
        }
        encoded.push('%');
        encoded.push(hex_char(byte >> 4));
        encoded.push(hex_char(byte & 0x0F));
    }
    encoded
}

/// Upper-case hex for file-name escapes; the historical encoding every
/// existing state root's file names use.
fn hex_char(nibble: u8) -> char {
    const HEX_DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
    ];
    HEX_DIGITS
        .get(usize::from(nibble))
        .copied()
        .map_or('0', std::convert::identity)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoMapDocType, RepoMapExactnessSummary,
        RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapRedactionState,
        RepoMapSnapshotMeta, RevisionId,
    };
    use tempfile::tempdir;

    use quanta_index_core::{
        CoreError, QUARANTINE_TARGET_NOT_QUARANTINED_CODE, QuarantineDiscardOutcomeV1,
        QuarantinedRepoMapFileV1,
    };

    use super::{
        AFTER_RENAME, AFTER_TEMPORARY_SYNC, CRASH_BOUNDARY_ENV, CRASH_EXIT_CODE,
        QUARANTINE_REASON_NOT_RECORDED, QUARANTINE_REASON_SUFFIX, RepoMapActivationRecordV1,
        RepoMapSnapshotPersistence, TEMPORARY_PREFIX, quarantine_reason_path, snapshot_file_name,
    };
    use crate::model::{RepoMapEntry, RepoMapSnapshot};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const CRASH_ROOT_ENV: &str = "QUANTA_INDEX_REPOMAP_CRASH_ROOT";

    fn fixture_snapshot(generation: u64, marker: &str) -> RepoMapSnapshot {
        let mut contributing_signals = BTreeMap::new();
        let _prior = contributing_signals.insert("files".to_string(), 3);
        RepoMapSnapshot {
            repo_id: RepoId::new("repo/a"),
            revision_id: RevisionId::new("rev:b"),
            manifest_generation: ManifestGeneration::new(generation),
            snapshot_meta: RepoMapSnapshotMeta {
                snapshot_id: format!("snapshot-{marker}"),
                projection_version: 2,
                authority_digest: "authority-1".to_string(),
                item_index_availability: RepoMapItemIndexAvailability::Full,
                graph_coverage_class: RepoMapGraphCoverageClass::Full,
                exactness_summary: RepoMapExactnessSummary::Exact,
            },
            entries: vec![RepoMapEntry {
                subject_identity: "subject://main".to_string(),
                subject_doc_type: RepoMapDocType::File,
                subject_kind: "File".to_string(),
                owner_path: "src/main.rs".to_string(),
                score: 0.75,
                final_score_millis: 750,
                importance_score_millis: 800,
                utility_score_millis: 700,
                freshness_score_millis: 650,
                evidence_priority_millis: 600,
                token_budget_hint: 512,
                contributing_signals,
                projection_evidence_kind: "bundle".to_string(),
                projection_authority_artifact_id: "artifact-1".to_string(),
                projection_authority_digest: "digest-1".to_string(),
                projection_status: "fresh".to_string(),
                redaction_state: RepoMapRedactionState::Unredacted,
                search_text: "main file".to_string(),
                source_symbol_count: 2,
                source_chunk_token_total: 120,
                source_call_incoming_edges: 4,
                source_call_outgoing_edges: 5,
                source_import_incoming_edges: 6,
                source_import_outgoing_edges: 7,
            }],
        }
    }

    #[test]
    fn activation_record_v1_round_trip_json_and_refuses_unknown_fields() -> TestResult {
        let record = RepoMapActivationRecordV1 {
            repo_id: "repo/a".to_string(),
            revision_id: "rev:b".to_string(),
            manifest_generation: 11,
        };
        let encoded = serde_json::to_value(&record)?;
        let decoded: RepoMapActivationRecordV1 = serde_json::from_value(encoded)?;
        assert_eq!(decoded, record);
        let forged = serde_json::json!({
            "repo_id": "repo/a", "revision_id": "rev:b", "manifest_generation": 11, "extra": 1
        });
        assert!(serde_json::from_value::<RepoMapActivationRecordV1>(forged).is_err());
        Ok(())
    }

    #[test]
    fn snapshot_and_activation_round_trip_under_the_digest_envelope() -> TestResult {
        let root = tempdir()?;
        let persistence = RepoMapSnapshotPersistence::open(root.path())?;
        let snapshot = fixture_snapshot(11, "one");
        persistence.persist_snapshot(&snapshot)?;
        persistence.persist_activation(
            &snapshot.repo_id,
            &snapshot.revision_id,
            snapshot.manifest_generation.get(),
        )?;
        let loaded = persistence.load()?;
        assert_eq!(loaded.snapshots, vec![snapshot]);
        assert_eq!(
            loaded.activations,
            vec![RepoMapActivationRecordV1 {
                repo_id: "repo/a".to_string(),
                revision_id: "rev:b".to_string(),
                manifest_generation: 11,
            }]
        );
        assert_eq!(loaded.report.snapshots_loaded, 1);
        assert_eq!(loaded.report.snapshots_migrated, 0);
        assert!(loaded.report.quarantined.is_empty());
        // The file is an envelope with a digest, not a bare snapshot.
        let bytes = std::fs::read(
            root.path()
                .join("snapshots")
                .join(snapshot_file_name(&fixture_snapshot(11, "one"))),
        )?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        assert_eq!(value.get("format_version"), Some(&serde_json::json!(2)));
        assert!(
            value
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|hex| hex.len() == 64)
        );
        Ok(())
    }

    #[test]
    fn a_legacy_bare_snapshot_is_migrated_once_and_verified_after() -> TestResult {
        let root = tempdir()?;
        let persistence = RepoMapSnapshotPersistence::open(root.path())?;
        let snapshot = fixture_snapshot(3, "legacy");
        let path = root
            .path()
            .join("snapshots")
            .join(snapshot_file_name(&snapshot));
        std::fs::write(&path, serde_json::to_vec_pretty(&snapshot)?)?;
        let first = persistence.load()?;
        assert_eq!(first.snapshots, vec![snapshot.clone()]);
        assert_eq!(first.report.snapshots_migrated, 1);
        let second = persistence.load()?;
        assert_eq!(second.snapshots, vec![snapshot]);
        assert_eq!(second.report.snapshots_migrated, 0);
        assert_eq!(second.report.snapshots_loaded, 1);
        let rewritten: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        assert!(rewritten.get("sha256").is_some());
        Ok(())
    }

    #[test]
    fn a_damaged_file_is_quarantined_with_its_reason_and_the_rest_opens() -> TestResult {
        let root = tempdir()?;
        let persistence = RepoMapSnapshotPersistence::open(root.path())?;
        let healthy = fixture_snapshot(1, "healthy");
        let rotted = fixture_snapshot(2, "rotted");
        let truncated = fixture_snapshot(3, "truncated");
        for snapshot in [&healthy, &rotted, &truncated] {
            persistence.persist_snapshot(snapshot)?;
        }
        persistence.persist_activation(&healthy.repo_id, &healthy.revision_id, 1)?;
        let snapshots_dir = root.path().join("snapshots");
        // A byte inside the snapshot body flips: the envelope still parses,
        // the digest does not match.
        let rotted_path = snapshots_dir.join(snapshot_file_name(&rotted));
        let text = std::fs::read_to_string(&rotted_path)?;
        let flipped = text.replacen("\"snapshot-rotted\"", "\"snapshot-rottex\"", 1);
        assert_ne!(text, flipped, "the fixture marker must be in the file");
        std::fs::write(&rotted_path, flipped)?;
        // A file cut short mid-write by something that was not this store.
        let truncated_path = snapshots_dir.join(snapshot_file_name(&truncated));
        let bytes = std::fs::read(&truncated_path)?;
        let half = bytes.len().checked_div(2).ok_or("halve the file")?;
        std::fs::write(
            &truncated_path,
            bytes.get(..half).ok_or("half is in range")?,
        )?;
        // An activation that is not an activation.
        let bogus_activation = root.path().join("activations").join("bogus--file.json");
        std::fs::write(&bogus_activation, b"{\"nope\": true}")?;

        let loaded = persistence.load()?;
        assert_eq!(loaded.snapshots, vec![healthy]);
        assert_eq!(loaded.activations.len(), 1);
        let mut quarantined: Vec<(String, bool, bool)> = loaded
            .report
            .quarantined
            .iter()
            .map(|entry| {
                (
                    entry.file_name.clone(),
                    entry.reason.contains("digest"),
                    entry.reason.contains("decode"),
                )
            })
            .collect();
        quarantined.sort();
        assert_eq!(
            quarantined,
            vec![
                ("bogus--file.json".to_string(), false, true),
                (snapshot_file_name(&rotted), true, false),
                (snapshot_file_name(&truncated), false, true),
            ]
        );
        for entry in &loaded.report.quarantined {
            assert!(
                root.path()
                    .join("quarantine")
                    .join(&entry.file_name)
                    .is_file()
            );
            assert!(!snapshots_dir.join(&entry.file_name).exists());
        }
        // The next open finds a clean store.
        let again = persistence.load()?;
        assert!(again.report.quarantined.is_empty());
        assert_eq!(again.snapshots.len(), 1);
        Ok(())
    }

    /// QI-BB-026 follow-up: the quarantine is discarded only as listed.
    ///
    /// The listing is live and carries the reason recorded beside each
    /// file; a discard names the file and that reason and removes both
    /// files; a stale reason, a nested name, a reason sidecar named
    /// directly or a directory removes nothing and is refused typed; a
    /// name already gone is `Absent`; a file quarantined by a build that
    /// recorded no reason is listed under the fixed sentence and discarded
    /// by naming it back.
    #[test]
    fn the_quarantine_is_listed_with_reasons_and_discarded_only_as_listed() -> TestResult {
        let root = tempdir()?;
        let persistence = RepoMapSnapshotPersistence::open(root.path())?;
        let rotted = fixture_snapshot(2, "rotted");
        persistence.persist_snapshot(&rotted)?;
        let snapshots_dir = root.path().join("snapshots");
        let rotted_path = snapshots_dir.join(snapshot_file_name(&rotted));
        let text = std::fs::read_to_string(&rotted_path)?;
        std::fs::write(
            &rotted_path,
            text.replacen("\"snapshot-rotted\"", "\"snapshot-rottex\"", 1),
        )?;
        let _loaded = persistence.load()?;
        let quarantine_dir = root.path().join("quarantine");
        // A file set aside by a build that recorded no reason.
        std::fs::write(quarantine_dir.join("old--file.json"), b"{}")?;
        // Things that live in the quarantine directory but are not
        // quarantined files: a directory, and the reason sidecars.
        std::fs::create_dir(quarantine_dir.join("a-directory"))?;

        let listed = persistence.quarantined_files()?;
        let names: Vec<&str> = listed
            .iter()
            .map(|entry| entry.file_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["old--file.json", snapshot_file_name(&rotted).as_str()]
        );
        let Some(old) = listed
            .iter()
            .find(|entry| entry.file_name == "old--file.json")
        else {
            return Err("the unrecorded file is listed".into());
        };
        assert_eq!(old.reason, QUARANTINE_REASON_NOT_RECORDED);
        let Some(listed_rotted) = listed
            .iter()
            .find(|entry| entry.file_name == snapshot_file_name(&rotted))
        else {
            return Err("the rotted snapshot is listed".into());
        };
        assert!(
            listed_rotted.reason.contains("digest"),
            "{}",
            listed_rotted.reason
        );
        assert_eq!(
            std::fs::read_to_string(quarantine_reason_path(
                &quarantine_dir,
                &listed_rotted.file_name
            ))?,
            listed_rotted.reason,
            "the listed reason is the recorded one"
        );

        let refused = |entry: QuarantinedRepoMapFileV1| -> TestResult {
            match persistence.discard_quarantined_file(&entry) {
                Err(CoreError::Typed { code, .. })
                    if code == QUARANTINE_TARGET_NOT_QUARANTINED_CODE =>
                {
                    Ok(())
                }
                other => Err(format!("{entry:?} must be refused typed, got {other:?}").into()),
            }
        };
        refused(QuarantinedRepoMapFileV1 {
            file_name: listed_rotted.file_name.clone(),
            reason: "some other reason".to_string(),
        })?;
        refused(QuarantinedRepoMapFileV1 {
            file_name: format!("../snapshots/{}", listed_rotted.file_name),
            reason: listed_rotted.reason.clone(),
        })?;
        refused(QuarantinedRepoMapFileV1 {
            file_name: format!("{}{QUARANTINE_REASON_SUFFIX}", listed_rotted.file_name),
            reason: listed_rotted.reason.clone(),
        })?;
        refused(QuarantinedRepoMapFileV1 {
            file_name: "a-directory".to_string(),
            reason: "anything".to_string(),
        })?;
        assert!(quarantine_dir.join(&listed_rotted.file_name).is_file());
        assert!(quarantine_reason_path(&quarantine_dir, &listed_rotted.file_name).is_file());
        assert!(quarantine_dir.join("a-directory").is_dir());
        assert_eq!(
            persistence.quarantined_files()?,
            listed,
            "a refused discard changes nothing"
        );

        let bytes = std::fs::metadata(quarantine_dir.join(&listed_rotted.file_name))?.len();
        assert_eq!(
            persistence.discard_quarantined_file(listed_rotted)?,
            QuarantineDiscardOutcomeV1::Discarded { bytes }
        );
        assert!(!quarantine_dir.join(&listed_rotted.file_name).exists());
        assert!(!quarantine_reason_path(&quarantine_dir, &listed_rotted.file_name).exists());
        assert_eq!(
            persistence.discard_quarantined_file(listed_rotted)?,
            QuarantineDiscardOutcomeV1::Absent
        );
        assert_eq!(
            persistence.discard_quarantined_file(old)?,
            QuarantineDiscardOutcomeV1::Discarded { bytes: 2 }
        );
        assert!(persistence.quarantined_files()?.is_empty());
        Ok(())
    }

    #[test]
    fn a_file_under_the_wrong_name_is_quarantined() -> TestResult {
        let root = tempdir()?;
        let persistence = RepoMapSnapshotPersistence::open(root.path())?;
        let snapshot = fixture_snapshot(5, "misnamed");
        persistence.persist_snapshot(&snapshot)?;
        let snapshots_dir = root.path().join("snapshots");
        std::fs::rename(
            snapshots_dir.join(snapshot_file_name(&snapshot)),
            snapshots_dir.join("other--name--g5.json"),
        )?;
        let loaded = persistence.load()?;
        assert!(loaded.snapshots.is_empty());
        assert_eq!(loaded.report.quarantined.len(), 1);
        assert!(
            loaded
                .report
                .quarantined
                .first()
                .is_some_and(|entry| entry.reason.contains("another file name"))
        );
        Ok(())
    }

    #[test]
    fn retention_removes_a_generation_and_tolerates_its_absence() -> TestResult {
        let root = tempdir()?;
        let persistence = RepoMapSnapshotPersistence::open(root.path())?;
        let snapshot = fixture_snapshot(9, "gone");
        persistence.persist_snapshot(&snapshot)?;
        persistence.remove_snapshot(
            &snapshot.repo_id,
            &snapshot.revision_id,
            snapshot.manifest_generation,
        )?;
        persistence.remove_snapshot(
            &snapshot.repo_id,
            &snapshot.revision_id,
            snapshot.manifest_generation,
        )?;
        assert!(persistence.load()?.snapshots.is_empty());
        Ok(())
    }

    fn crash_child_root() -> Option<std::path::PathBuf> {
        std::env::var_os(CRASH_ROOT_ENV).map(std::path::PathBuf::from)
    }

    /// The crash matrix.
    ///
    /// A child process rewrites generation 7's snapshot and is killed at each
    /// step of the write protocol; the parent then opens the store and must
    /// find exactly the previous snapshot or the new one, never a mix and
    /// never a failure.
    #[test]
    fn a_crash_at_every_write_step_leaves_the_previous_or_the_new_snapshot() -> TestResult {
        if let Some(root) = crash_child_root() {
            let persistence = RepoMapSnapshotPersistence::open(&root)?;
            persistence.persist_snapshot(&fixture_snapshot(7, "new"))?;
            return Err("the crash boundary did not fire in the child".into());
        }
        let test_name = "persistence::tests::a_crash_at_every_write_step_leaves_the_previous_or_the_new_snapshot";
        for (boundary, expect_new, expect_temporaries) in [
            (AFTER_TEMPORARY_SYNC, false, 1_u64),
            (AFTER_RENAME, true, 0_u64),
        ] {
            let temp = tempdir()?;
            let root = temp.path().to_path_buf();
            let persistence = RepoMapSnapshotPersistence::open(&root)?;
            persistence.persist_snapshot(&fixture_snapshot(7, "old"))?;
            let status = Command::new(std::env::current_exe()?)
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .env(CRASH_ROOT_ENV, &root)
                .env(CRASH_BOUNDARY_ENV, boundary)
                .status()?;
            assert_eq!(
                status.code(),
                Some(CRASH_EXIT_CODE),
                "the child must stop exactly at {boundary}; status={status}"
            );
            let mut temporaries = 0_usize;
            for entry in std::fs::read_dir(root.join("snapshots"))? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(TEMPORARY_PREFIX))
                {
                    temporaries = temporaries.saturating_add(1);
                }
            }
            assert_eq!(
                u64::try_from(temporaries)?,
                expect_temporaries,
                "temporaries left at {boundary}"
            );
            let loaded = RepoMapSnapshotPersistence::open(&root)?.load()?;
            assert!(
                loaded.report.quarantined.is_empty(),
                "nothing to quarantine at {boundary}"
            );
            assert_eq!(loaded.report.stale_temporaries_removed, expect_temporaries);
            let expected_marker = if expect_new {
                "snapshot-new"
            } else {
                "snapshot-old"
            };
            assert_eq!(
                loaded
                    .snapshots
                    .iter()
                    .map(|snapshot| snapshot.snapshot_meta.snapshot_id.as_str())
                    .collect::<Vec<_>>(),
                vec![expected_marker],
                "at {boundary}"
            );
            // The sweep is durable: a second open sees no temporaries.
            let again = RepoMapSnapshotPersistence::open(&root)?.load()?;
            assert_eq!(again.report.stale_temporaries_removed, 0);
        }
        Ok(())
    }
}
