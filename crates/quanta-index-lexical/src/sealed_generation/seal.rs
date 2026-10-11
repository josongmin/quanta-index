//! The seal: measure a finalized generation and write its manifest.
//!
//! Runs after the index writer has committed, waited for its merges and
//! been released, after the text authority has been brought to its
//! manifest's file set, and before the sealed identity is written — so a
//! crash in between leaves an unsealed generation, never a sealed one
//! without a manifest.
//!
//! Commitment hashing reads bytes proportional to what the generation changed, not
//! to its size (QI-BB-006 보완 #4): a file whose inode is the base
//! generation's — an index segment, a text-authority shard or an overlay
//! the delta inherited by hard link — carries the base manifest's
//! commitment without being read, because the base seal proved that very
//! inode; everything else — a new segment, a rewritten shard, the
//! rewritten `meta.json`, a rewritten overlay — is read once through a
//! counting hasher. Coverage pages are an exception: their inherited inodes
//! are hashed at seal because in-place changes must not bypass the prepared
//! coverage root. A fresh generation is therefore proved whole at its seal.
//!
//! V17 independently checks changed File sources against the original mutation
//! before publishing the identity. Immutable pool objects are authenticated by
//! the File and Native verifiers, separately from generation-local hashing.
//!
//! The measurements are reported per seal so a test can hold the seal to
//! that shape with an inode oracle instead of a clock.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::diagnostic_trace::DiagnosticBm25BoundaryLabelV1;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};
use sha2::{Digest, Sha256};
use tantivy::collector::Count;
use tantivy::query::TermQuery;
use tantivy::schema::IndexRecordOption;
use tantivy::{Index, IndexReader, ReloadPolicy, Term};

use crate::file_authority;
use crate::normalize::TEXT_NORMALIZER_VERSION;
use crate::overlay_codec::OverlayFamily;
use crate::ranked_keys::{self, MAX_RANKED_KEYS_BYTES};
use crate::sealed_generation::coverage::SOURCE_FILE_COVERAGE_FILE_NAME;
use crate::sealed_generation::index_files::referenced_index_files;
use crate::sealed_generation::live_bm25::{self, BaseSnapshot, LiveBm25Statistics};
use crate::sealed_generation::manifest::{
    IndexSegmentVerificationV1, LexicalSealedManifest, write_manifest,
};
use crate::text_authority::TEXT_AUTHORITY_DIR_NAME;
use crate::{SchemaFields, TANTIVY_INDEX_META_FILE_NAME, TEXT_DOC_KIND};

/// What the seals of one adapter read to commit their generations
/// (QI-BB-006 보완 #4).
///
/// Every generation-local committed file is counted under exactly one of two
/// sources. `bytes_hashed` is what the commitment hasher reads;
/// `bytes_inherited` bypasses that hasher. Verified File source input counts
/// are logical mutation bytes, separate from pool object physical I/O.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalSealCommitmentStats {
    /// Generations sealed.
    pub seals: u64,
    /// Files read and hashed by a seal.
    pub files_hashed: u64,
    /// Bytes read through the seal's hasher.
    pub bytes_hashed: u64,
    /// Files whose commitment was carried from the base generation's
    /// manifest because the file is the base's inode.
    pub files_inherited: u64,
    /// Their length, not read by the commitment hasher.
    pub bytes_inherited: u64,
    /// Upsert source inputs whose bytes were independently verified at seal.
    pub file_admission_sources_verified: u64,
    /// Logical raw bytes of those verified Upsert source inputs.
    pub file_admission_source_bytes_verified: u64,
}

impl LexicalSealCommitmentStats {
    /// Fold one seal's measurement into the running totals.
    pub(crate) fn absorb(&mut self, seal: Self) {
        self.seals = self.seals.saturating_add(seal.seals);
        self.files_hashed = self.files_hashed.saturating_add(seal.files_hashed);
        self.bytes_hashed = self.bytes_hashed.saturating_add(seal.bytes_hashed);
        self.files_inherited = self.files_inherited.saturating_add(seal.files_inherited);
        self.bytes_inherited = self.bytes_inherited.saturating_add(seal.bytes_inherited);
        self.file_admission_sources_verified = self
            .file_admission_sources_verified
            .saturating_add(seal.file_admission_sources_verified);
        self.file_admission_source_bytes_verified = self
            .file_admission_source_bytes_verified
            .saturating_add(seal.file_admission_source_bytes_verified);
    }

    fn hashed(&mut self, bytes: u64) {
        self.files_hashed = self.files_hashed.saturating_add(1);
        self.bytes_hashed = self.bytes_hashed.saturating_add(bytes);
    }

    fn inherited(&mut self, bytes: u64) {
        self.files_inherited = self.files_inherited.saturating_add(1);
        self.bytes_inherited = self.bytes_inherited.saturating_add(bytes);
    }
}

/// The base generation's commitments, for inheritance by inode.
struct BaseCommitments {
    dir: PathBuf,
    by_name: BTreeMap<String, SealedArtifactCommitmentV1>,
}

impl BaseCommitments {
    /// The base's manifest, if the base is sealed.
    ///
    /// An unsealed base (a delta on a generation whose own seal never came)
    /// or a base already reclaimed has nothing to inherit from; a base
    /// whose manifest cannot be read is an error, not a reason to hash
    /// quietly.
    fn read(base_dir: Option<&Path>) -> Result<Option<Self>, CoreError> {
        let Some(base_dir) = base_dir else {
            return Ok(None);
        };
        if !crate::index_store::sealed_identity_entry_present(base_dir)? {
            return Ok(None);
        }
        let opened = super::open_generation_dir_nofollow(base_dir).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: open commitment base {}: {error}",
                base_dir.display()
            ))
        })?;
        let identity = crate::index_store::read_lexical_sealed_identity_at(base_dir, &opened)?;
        let manifest =
            super::manifest::read_bound_manifest_at(base_dir, &opened, &identity.manifest_digest)?;
        let mut by_name: BTreeMap<_, _> = manifest
            .all_commitments()
            .map(|artifact| (artifact.name.clone(), artifact.clone()))
            .collect();
        if let Some(root) = &manifest.source_coverage {
            for page in crate::sealed_generation::coverage::root_page_commitments_at(
                &opened, base_dir, root, &identity,
            )? {
                let _previous = by_name.insert(page.name.clone(), page);
            }
        }
        Ok(Some(Self {
            dir: base_dir.to_path_buf(),
            by_name,
        }))
    }

    /// The base's commitment for `name`, if `path` is the very inode the
    /// base committed to.
    fn inherited(
        &self,
        name: &str,
        target_dir: &Path,
    ) -> Result<Option<SealedArtifactCommitmentV1>, CoreError> {
        let Some(listed) = self.by_name.get(name) else {
            return Ok(None);
        };
        let path = target_dir.join(name);
        let ours = super::open_regular_nofollow(target_dir, Path::new(name))
            .and_then(|file| file.metadata())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: inspect {} for commitment: {error}",
                    path.display()
                ))
            })?;
        let theirs = match super::open_regular_nofollow(&self.dir, Path::new(name))
            .and_then(|file| file.metadata())
        {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect base generation entry {} for commitment: {error}",
                    self.dir.join(name).display()
                )));
            }
        };
        let same_inode = ours.is_file()
            && theirs.is_file()
            && ours.dev() == theirs.dev()
            && ours.ino() == theirs.ino();
        if same_inode && ours.len() == listed.bytes {
            return Ok(Some(listed.clone()));
        }
        Ok(None)
    }
}

/// One seal in progress: its measurement and where it may inherit from.
struct Measurer {
    generation_dir: PathBuf,
    base: Option<BaseCommitments>,
    stats: LexicalSealCommitmentStats,
}

impl Measurer {
    /// Length and SHA-256 of `name`, read once through a fixed buffer and
    /// counted.
    fn hash(&mut self, name: &str) -> Result<SealedArtifactCommitmentV1, CoreError> {
        let path = self.generation_dir.join(name);
        let mut file = super::open_regular_nofollow(&self.generation_dir, Path::new(name))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound
                    || super::is_unsafe_artifact_path(&error)
                {
                    crate::index_store::sidecar_corrupt(
                        &self.generation_dir,
                        name,
                        "changed before open",
                    )
                } else {
                    CoreError::Storage(format!(
                        "lexical: open {} for commitment: {error}",
                        path.display()
                    ))
                }
            })?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1 << 16];
        let mut length = 0_u64;
        loop {
            let read = file.read(&mut buffer).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: read {} for commitment: {error}",
                    path.display()
                ))
            })?;
            if read == 0 {
                break;
            }
            let chunk = buffer.get(..read).ok_or_else(|| {
                CoreError::Storage(format!(
                    "lexical: read {} for commitment: read returned more bytes than the buffer holds",
                    path.display()
                ))
            })?;
            hasher.update(chunk);
            length = length
                .checked_add(crate::channel_payloads::count_from_len(read)?)
                .ok_or_else(|| {
                    CoreError::Storage(format!("lexical: {} length overflows u64", path.display()))
                })?;
        }
        self.stats.hashed(length);
        Ok(SealedArtifactCommitmentV1 {
            name: name.to_string(),
            bytes: length,
            sha256: hasher.finalize().into(),
        })
    }

    /// The commitment for `name`: the base's if the file is the base's
    /// inode, otherwise measured.
    fn commit(&mut self, name: &str) -> Result<SealedArtifactCommitmentV1, CoreError> {
        if let Some(base) = &self.base
            && let Some(inherited) = base.inherited(name, &self.generation_dir)?
        {
            self.stats.inherited(inherited.bytes);
            return Ok(inherited);
        }
        self.hash(name)
    }
}

/// One marker for a bounded lexical owner call envelope. `logical_bytes`
/// names explicit payload bytes already read or produced; it is not device I/O.
#[expect(
    clippy::print_stderr,
    reason = "bounded opt-in causal marker is replayed against the scale artifact"
)]
fn emit_bm25_boundary(
    label: DiagnosticBm25BoundaryLabelV1,
    started: Option<Instant>,
    logical_bytes: Option<usize>,
    index_files: usize,
    legacy_enabled: bool,
    diagnostic: Option<&DiagnosticContextV1>,
    span: Option<DiagnosticSpanGuardV1>,
) {
    let Some(started) = started else {
        return;
    };
    let counts = logical_bytes.and_then(|bytes| {
        let (Ok(bytes), Ok(index_files), Ok(elapsed_ns)) = (
            u64::try_from(bytes),
            u64::try_from(index_files),
            u64::try_from(started.elapsed().as_nanos()),
        ) else {
            return None;
        };
        Some((bytes, index_files, elapsed_ns))
    });
    if let Some((logical_bytes, index_files, elapsed_ns)) = counts {
        if let (Some(context), Some(span)) = (diagnostic, span) {
            context.bm25_boundary_aggregate(
                &span,
                label,
                1,
                elapsed_ns,
                logical_bytes,
                index_files,
            );
            let _finished = span.finish(DiagnosticTerminalV1::Ok);
        }
        if legacy_enabled {
            eprintln!(
                "QI_CAUSAL_V1 kind=bm25_boundary label={} ok=1 elapsed_ns={elapsed_ns} logical_bytes={logical_bytes} index_files={index_files}",
                label.as_str(),
            );
        }
    } else {
        if let Some(context) = diagnostic {
            context.health(DiagnosticHealthCodeV1::CounterOverflow);
        }
        if legacy_enabled {
            eprintln!(
                "QI_CAUSAL_V1 kind=bm25_boundary label={} ok=0 reason=counter_overflow",
                label.as_str(),
            );
        }
    }
}

/// Read the committed base files needed for exact live-statistics inheritance.
///
/// This custody belongs to the seal door; the statistics
/// builder receives a verified capability and has no index-store dependency.
fn read_bm25_base(
    dir: &Path,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<Option<BaseSnapshot>, CoreError> {
    if !crate::index_store::sealed_identity_entry_present(dir)? {
        return Ok(None);
    }
    let span = diagnostic.and_then(|context| {
        crate::causal_profile::enter_span(context, DiagnosticStageV1::Bm25Boundary)
    });
    let legacy_enabled = crate::causal_profile::enabled();
    let started = (legacy_enabled || span.is_some()).then(Instant::now);
    let root = super::open_generation_dir_nofollow(dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: open BM25 base {}: {error}",
            dir.display()
        ))
    })?;
    let identity = crate::index_store::read_lexical_sealed_identity_at(dir, &root)?;
    let manifest = super::manifest::read_bound_manifest_at(dir, &root, &identity.manifest_digest)?;
    let meta = super::verify::read_committed(&root, dir, &manifest.index_meta, None)?;
    let meta_len = meta.len();
    let index = crate::index_store::open_sealed_index_at(dir, &root, meta)?;
    super::verify::verify_index_segments(&root, dir, &index, &manifest.index_segments)?;
    let bytes = super::verify::read_committed(&root, dir, &manifest.live_bm25, None)?;
    let reader = index
        .reader()
        .map_err(|error| CoreError::Storage(format!("lexical: read BM25 base index: {error}")))?;
    let statistics =
        LiveBm25Statistics::decode(&bytes, dir, manifest.index_meta.sha256, &reader.searcher())?;
    emit_bm25_boundary(
        DiagnosticBm25BoundaryLabelV1::BaseRead,
        started,
        meta_len.checked_add(bytes.len()),
        manifest.index_segments.len(),
        legacy_enabled,
        diagnostic,
        span,
    );
    Ok(Some(BaseSnapshot {
        statistics,
        index,
        index_segments: manifest.index_segments,
    }))
}

/// Measure `generation_dir` as sealed and write its manifest.
///
/// `base_dir` is the delta base the generation was carried forward from,
/// if any. Returns what this seal read and did not have to read.
pub(crate) fn seal_generation(
    generation_dir: &Path,
    fields: &SchemaFields,
    identity: &GenerationSnapshot,
    base_dir: Option<&Path>,
    prepared_coverage_root: &SealedArtifactCommitmentV1,
    publication: &crate::lexical_publication::PublicationContext,
    sync: Option<&crate::causal_profile::SyncTotalsV1>,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<(LexicalSealCommitmentStats, u64), CoreError> {
    crate::index_store::remove_publish_leftovers(
        generation_dir,
        quanta_index_core::DiagnosticSyncLabelV1::SealedLeftoverDirectory,
        sync,
    )?;
    let mut measurer = Measurer {
        generation_dir: generation_dir.to_path_buf(),
        base: BaseCommitments::read(base_dir)?,
        stats: LexicalSealCommitmentStats {
            seals: 1,
            ..LexicalSealCommitmentStats::default()
        },
    };
    let index = crate::index_store::open_sealed_index(generation_dir)?;
    let index_meta = measurer.hash(TANTIVY_INDEX_META_FILE_NAME)?;
    let mut index_segments = Vec::new();
    for name in referenced_index_files(&index, generation_dir)? {
        index_segments.push(measurer.commit(&name)?);
    }
    let base_statistics = base_dir
        .map(|dir| read_bm25_base(dir, diagnostic))
        .transpose()?
        .flatten();
    let live_statistics = LiveBm25Statistics::build_observed(
        &index,
        fields,
        index_meta.sha256,
        generation_dir,
        base_statistics.as_ref(),
        &index_segments,
        diagnostic,
    )?;
    let encode_span = diagnostic.and_then(|context| {
        crate::causal_profile::enter_span(context, DiagnosticStageV1::Bm25Boundary)
    });
    let legacy_enabled = crate::causal_profile::enabled();
    let encode_started = (legacy_enabled || encode_span.is_some()).then(Instant::now);
    let live_bytes = live_statistics.encode()?;
    emit_bm25_boundary(
        DiagnosticBm25BoundaryLabelV1::Encode,
        encode_started,
        Some(live_bytes.len()),
        index_segments.len(),
        legacy_enabled,
        diagnostic,
        encode_span,
    );
    crate::index_store::write_atomic_durable_observed(
        &generation_dir.join(live_bm25::FILE_NAME),
        &live_bytes,
        "live BM25 statistics",
        sync,
    )?;
    let live_bm25 = measurer.hash(live_bm25::FILE_NAME)?;
    let ranked_keys = commit_ranked_keys(&index, &mut measurer, sync)?;
    let file_admission_started = Instant::now();
    let native = crate::lexical_publication::read_native_root(generation_dir)?;
    if publication.native_root.as_ref() != Some(&native) {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            crate::lexical_publication::NATIVE_ROOT_NAME,
            "root differs from publication witness",
        ));
    }
    let native_mutation = publication
        .native_mutation
        .as_ref()
        .ok_or_else(|| CoreError::InvalidContract("Native publication witness missing".into()))?;
    crate::causal_profile::timed_work("seal_native_transition", || {
        crate::text_authority::verify_transition_for_seal(
            &publication.store,
            &native,
            publication.native_base.as_ref(),
            &publication.reservation.native_text,
            &native_mutation.sources,
            &native_mutation.retired,
            &native_mutation.added,
            native_mutation.clear_all,
            publication
                .native_base
                .as_ref()
                .map_or(1, |root| root.next_segment_id),
            None,
        )
    })?;
    crate::causal_profile::timed_work("seal_native_changed_segments", || {
        crate::text_authority::verify_changed_segments_for_seal(
            &publication.store,
            &native,
            publication
                .native_base
                .as_ref()
                .map_or(1, |root| root.next_segment_id),
        )
    })?;
    crate::causal_profile::timed_work("seal_native_index_coverage", || {
        ensure_text_authority_covers_index(&index, generation_dir, fields, &native)
    })?;
    let text_authority = Some(vec![
        measurer.hash(crate::lexical_publication::NATIVE_ROOT_NAME)?,
    ]);
    let file_bytes = crate::lexical_publication::read_root_bytes(
        generation_dir,
        crate::lexical_publication::FILE_ROOT_NAME,
    )?;
    let file_root = file_authority::paged::AuthorityRoot::decode(&file_bytes)?;
    let mutation = publication
        .file_mutation
        .as_ref()
        .ok_or_else(|| CoreError::InvalidContract("File publication witness missing".into()))?;
    if publication.file_root.as_ref() != Some(&file_root)
        || mutation.operations_sha256 != publication.operations_sha256
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            crate::lexical_publication::FILE_ROOT_NAME,
            "root or mutation differs from publication witness",
        ));
    }
    crate::causal_profile::timed_work("seal_file_transition_and_segments", || {
        file_authority::paged::verify_for_seal(
            &publication.store,
            &file_root,
            publication.file_base.as_ref(),
            &publication.reservation.file,
            mutation,
        )
    })?;
    for edit in &mutation.edits {
        if let file_authority::paged::FileSourceEdit::Upsert { bytes, .. } = edit {
            measurer.stats.file_admission_sources_verified = measurer
                .stats
                .file_admission_sources_verified
                .checked_add(1)
                .ok_or_else(|| {
                    CoreError::InvalidContract("verified source count overflow".into())
                })?;
            measurer.stats.file_admission_source_bytes_verified = measurer
                .stats
                .file_admission_source_bytes_verified
                .checked_add(u64::try_from(bytes.len()).map_err(|error| {
                    CoreError::InvalidContract(format!("verified source byte width: {error}"))
                })?)
                .ok_or_else(|| {
                    CoreError::InvalidContract("verified source bytes overflow".into())
                })?;
        }
    }
    let file_admission_ns = crate::stage_timing::elapsed_stage_ns(file_admission_started)?;
    let file_authority = vec![measurer.hash(crate::lexical_publication::FILE_ROOT_NAME)?];
    let mut overlays = Vec::new();
    for family in OverlayFamily::ALL {
        let path = family.path(generation_dir);
        match super::optional_entry_metadata(&path) {
            Ok(Some(metadata)) if metadata.is_file() => {
                overlays.push(measurer.commit(family.file_name())?);
            }
            Ok(Some(_)) => {
                return Err(crate::index_store::sidecar_corrupt(
                    generation_dir,
                    family.file_name(),
                    "overlay is not a regular file",
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect overlay {} before sealing: {error}",
                    path.display()
                )));
            }
        }
    }
    let source_coverage =
        match std::fs::symlink_metadata(generation_dir.join(SOURCE_FILE_COVERAGE_FILE_NAME)) {
            Ok(metadata) if metadata.is_file() => {
                Some(measurer.hash(SOURCE_FILE_COVERAGE_FILE_NAME)?)
            }
            Ok(_) => {
                return Err(crate::index_store::sidecar_corrupt(
                    generation_dir,
                    SOURCE_FILE_COVERAGE_FILE_NAME,
                    "coverage is not a regular file",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect coverage before sealing: {error}",
                )));
            }
        };
    let root = source_coverage.as_ref().ok_or_else(|| {
        crate::index_store::sidecar_corrupt(
            generation_dir,
            SOURCE_FILE_COVERAGE_FILE_NAME,
            "prepared coverage root is missing",
        )
    })?;
    if root != prepared_coverage_root {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            SOURCE_FILE_COVERAGE_FILE_NAME,
            "coverage root changed after preparation",
        ));
    }
    // The writer encoded touched rows from a validated plan and linked only
    // pages inherited from the verified base. Bind that plan to the persisted
    // root, then hash every page against it: an in-place edit of a hard-linked
    // base page must not inherit a stale manifest commitment.
    for page in
        crate::sealed_generation::coverage::root_page_commitments(generation_dir, root, identity)?
    {
        let page_commitment = measurer.hash(&page.name)?;
        if page_commitment != page {
            return Err(crate::index_store::sidecar_corrupt(
                generation_dir,
                &page.name,
                "coverage page changed between preparation and seal",
            ));
        }
    }
    let manifest = LexicalSealedManifest {
        manifest_digest: identity.manifest_digest.clone(),
        normalizer: TEXT_NORMALIZER_VERSION,
        index_meta,
        index_segment_verification: IndexSegmentVerificationV1::LengthAtOpenContentAtSealAndScrub,
        index_segments,
        ranked_keys,
        live_bm25,
        text_authority,
        file_authority,
        overlays,
        source_coverage,
    };
    write_manifest(generation_dir, &manifest, sync)?;
    Ok((measurer.stats, file_admission_ns))
}

/// Build only new segment tables.
///
/// A delta inherits the exact inode and commitment of an unchanged segment's
/// table. Remove tables for merged or deleted segments so an unlisted sidecar
/// cannot masquerade as authority. The bytes are capped at open.
fn commit_ranked_keys(
    index: &Index,
    measurer: &mut Measurer,
    sync: Option<&crate::causal_profile::SyncTotalsV1>,
) -> Result<Vec<SealedArtifactCommitmentV1>, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|error| CoreError::Storage(format!("lexical: ranked-key seal reader: {error}")))?;
    let searcher = reader.searcher();
    let mut segments: Vec<(String, &tantivy::SegmentReader)> = searcher
        .segment_readers()
        .iter()
        .map(|segment| (ranked_keys::file_name(segment), segment))
        .collect();
    segments.sort_by(|left, right| left.0.cmp(&right.0));
    if segments.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left.0 == right.0)
    }) {
        return Err(CoreError::Storage(
            "lexical: duplicate ranked-key segment".into(),
        ));
    }
    let expected: std::collections::BTreeSet<&str> =
        segments.iter().map(|(name, _)| name.as_str()).collect();
    for entry in std::fs::read_dir(&measurer.generation_dir)
        .map_err(|error| CoreError::Storage(format!("lexical: list ranked-key files: {error}")))?
    {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!("lexical: read ranked-key entry: {error}"))
        })?;
        let entry_name = entry.file_name();
        let name = entry_name.to_string_lossy();
        if ranked_keys::is_ranked_key_entry(&name) && !expected.contains(name.as_ref()) {
            std::fs::remove_file(entry.path()).map_err(|error| {
                CoreError::Storage(format!("lexical: remove stale ranked-key table: {error}"))
            })?;
        }
    }
    let mut commitments = Vec::new();
    let mut total = 0_u64;
    let resident_limit = u64::try_from(MAX_RANKED_KEYS_BYTES).map_err(|error| {
        CoreError::Storage(format!("lexical: ranked-key limit overflow: {error}"))
    })?;
    for (name, segment) in segments {
        let path = measurer.generation_dir.join(&name);
        let existing = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => true,
            Ok(_) => {
                return Err(crate::index_store::sidecar_corrupt(
                    &measurer.generation_dir,
                    &name,
                    "ranked-key table is not a regular file",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical: inspect ranked-key table {}: {error}",
                    path.display()
                )));
            }
        };
        let inherited = if existing {
            measurer
                .base
                .as_ref()
                .map(|base| base.inherited(&name, &measurer.generation_dir))
                .transpose()?
                .flatten()
        } else {
            None
        };
        if inherited.is_none() {
            let bytes = ranked_keys::encode(segment)?;
            crate::index_store::write_atomic_durable_observed(&path, &bytes, "ranked keys", sync)?;
        }
        let commitment = measurer.commit(&name)?;
        total = total
            .checked_add(commitment.bytes)
            .ok_or_else(|| CoreError::Storage("lexical: ranked-key table size overflow".into()))?;
        if total > resident_limit {
            return Err(CoreError::Storage(
                "lexical: ranked-key tables exceed resident limit".into(),
            ));
        }
        commitments.push(commitment);
    }
    Ok(commitments)
}

/// How many live text documents the committed index holds.
fn live_text_doc_count(index: &Index, fields: &SchemaFields) -> Result<u64, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|err| CoreError::Storage(format!("lexical: text doc count reader: {err}")))?;
    let query = TermQuery::new(
        Term::from_field_text(fields.doc_kind, TEXT_DOC_KIND),
        IndexRecordOption::Basic,
    );
    let count = reader
        .searcher()
        .search(&query, &Count)
        .map_err(|err| CoreError::Storage(format!("lexical: text doc count: {err}")))?;
    crate::channel_payloads::count_from_len(count)
}

/// The seal's coverage proof: the text authority lists exactly as many
/// documents as the index holds live text documents.
///
/// The two are published separately (Tantivy commit, then the Native root), so
/// a publish that crashed in between leaves the index ahead of the
/// authority; sealing that would serve keyword hits a regex or phrase can
/// never see. The count is one term query, never a scan.
fn ensure_text_authority_covers_index(
    index: &Index,
    generation_dir: &Path,
    fields: &SchemaFields,
    manifest: &crate::text_authority::NativeRoot,
) -> Result<(), CoreError> {
    let live = live_text_doc_count(index, fields)?;
    let listed = manifest.live_docs;
    if live != listed {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            TEXT_AUTHORITY_DIR_NAME,
            &format!("lists {listed} documents but the index holds {live} live text documents"),
        ));
    }
    Ok(())
}
