use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    ChannelSeq, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneActivateGenerationRequest, SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;

type SharedLedger = Arc<RwLock<Ledger>>;
type SharedActivationCatalog = Arc<ActivationCatalog>;

/// Per-track readiness state.
#[derive(Debug, Default)]
pub struct TrackLedger {
    sealed: Option<ManifestGeneration>,
    last_seen: ChannelSeq,
}

impl TrackLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn sealed(&self) -> Option<ManifestGeneration> {
        self.sealed
    }

    #[must_use]
    pub fn last_seen(&self) -> ChannelSeq {
        self.last_seen
    }

    /// Monotonic seal update. Lower generations do not rewind readiness.
    pub fn record_seal(&mut self, generation: ManifestGeneration) {
        let next = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(next);
    }

    pub fn set_last_seen(&mut self, seq: ChannelSeq) {
        self.last_seen = seq;
    }
}

/// Shared in-memory readiness ledger for query/readiness gating and channel replay.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical: TrackLedger,
    semantic: TrackLedger,
}

impl Ledger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn shared() -> SharedLedger {
        Arc::new(RwLock::new(Self::default()))
    }

    #[must_use]
    pub fn lexical(&self) -> &TrackLedger {
        &self.lexical
    }

    #[must_use]
    pub fn semantic(&self) -> &TrackLedger {
        &self.semantic
    }

    pub fn lexical_mut(&mut self) -> &mut TrackLedger {
        &mut self.lexical
    }

    pub fn semantic_mut(&mut self) -> &mut TrackLedger {
        &mut self.semantic
    }

    pub fn lexical_seal(&mut self, generation: ManifestGeneration) {
        self.lexical.record_seal(generation);
    }

    pub fn semantic_seal(&mut self, generation: ManifestGeneration) {
        self.semantic.record_seal(generation);
    }

    #[must_use]
    pub fn lexical_sealed(&self) -> Option<ManifestGeneration> {
        self.lexical.sealed()
    }

    #[must_use]
    pub fn semantic_sealed(&self) -> Option<ManifestGeneration> {
        self.semantic.sealed()
    }

    #[must_use]
    pub fn lexical_last_seen(&self) -> ChannelSeq {
        self.lexical.last_seen()
    }

    #[must_use]
    pub fn semantic_last_seen(&self) -> ChannelSeq {
        self.semantic.last_seen()
    }

    pub fn set_lexical_last_seen(&mut self, seq: ChannelSeq) {
        self.lexical.set_last_seen(seq);
    }

    pub fn set_semantic_last_seen(&mut self, seq: ChannelSeq) {
        self.semantic.set_last_seen(seq);
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActivationKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGenerationRecord {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub track: SearchPlaneTrackKind,
}

#[derive(Debug)]
pub struct ActivationCatalog {
    activations_dir: PathBuf,
    entries: RwLock<BTreeMap<ActivationKey, ActiveGenerationRecord>>,
}

impl ActivationCatalog {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: create root {}: {err}",
                root.display()
            ))
        })?;
        let mut entries = BTreeMap::new();
        let dir_entries = fs::read_dir(root).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: list root {}: {err}",
                root.display()
            ))
        })?;
        for entry in dir_entries {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read dir entry in {}: {err}",
                    root.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: inspect dir entry in {}: {err}",
                    root.display()
                ))
            })?;
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|value| value != "json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read activation {}: {err}",
                    path.display()
                ))
            })?;
            let request = serde_json::from_slice::<SearchPlaneActivateGenerationRequest>(&bytes)
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-plane activation catalog: decode activation {}: {err}",
                        path.display()
                    ))
                })?;
            for track in &request.tracks {
                insert_activation_record(&mut entries, &request, *track);
            }
        }
        Ok(Self {
            activations_dir: root.to_path_buf(),
            entries: RwLock::new(entries),
        })
    }

    pub fn shared(root: impl AsRef<Path>) -> Result<SharedActivationCatalog, CoreError> {
        Ok(Arc::new(Self::open(root)?))
    }

    pub fn activate(
        &self,
        request: &SearchPlaneActivateGenerationRequest,
    ) -> Result<(), CoreError> {
        if request.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "activate-generation: manifest_digest must not be empty".to_string(),
            ));
        }
        if request.tracks.is_empty() {
            return Err(CoreError::InvalidContract(
                "activate-generation: tracks must not be empty".to_string(),
            ));
        }
        let mut entries = self.entries.write().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        for track in &request.tracks {
            let persisted = SearchPlaneActivateGenerationRequest {
                repo_id: request.repo_id.clone(),
                revision_id: request.revision_id.clone(),
                manifest_generation: request.manifest_generation,
                manifest_digest: request.manifest_digest.clone(),
                tracks: vec![*track],
            };
            let path = self.activations_dir.join(activation_file_name(
                &request.repo_id,
                &request.revision_id,
                *track,
            ));
            let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: encode activation {}: {err}",
                    path.display()
                ))
            })?;
            fs::write(&path, bytes).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: write activation {}: {err}",
                    path.display()
                ))
            })?;
            insert_activation_record(&mut entries, request, *track);
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<GenerationPin, CoreError> {
        let key = ActivationKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        let record = {
            let entries = self.entries.read().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            entries.get(&key).cloned().ok_or_else(|| {
                CoreError::NotReady(format!(
                    "activate-generation: no active {track:?} generation for repo={} revision={}",
                    repo_id.as_str(),
                    revision_id.as_str()
                ))
            })?
        };
        Ok(GenerationPin::new(
            record.repo_id,
            record.revision_id,
            record.manifest_generation,
        ))
    }
}

fn insert_activation_record(
    entries: &mut BTreeMap<ActivationKey, ActiveGenerationRecord>,
    request: &SearchPlaneActivateGenerationRequest,
    track: SearchPlaneTrackKind,
) {
    let key = ActivationKey {
        repo_id: request.repo_id.clone(),
        revision_id: request.revision_id.clone(),
        track,
    };
    let _prior = entries.insert(
        key,
        ActiveGenerationRecord {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            manifest_generation: request.manifest_generation,
            manifest_digest: request.manifest_digest.clone(),
            track,
        },
    );
}

fn activation_file_name(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    track: SearchPlaneTrackKind,
) -> String {
    format!(
        "{}--{}--{}.json",
        encode_component(repo_id.as_str()),
        encode_component(revision_id.as_str()),
        track.as_code_str(),
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
mod tests {
    use tempfile::tempdir;

    use quanta_index_contract::{
        ChannelSeq, ManifestGeneration, RepoId, RevisionId, SearchPlaneActivateGenerationRequest,
        SearchPlaneTrackKind,
    };

    use super::{ActivationCatalog, Ledger};

    #[test]
    fn seals_are_monotonic_per_track() {
        let mut ledger = Ledger::default();
        ledger.lexical_seal(ManifestGeneration::new(7));
        ledger.lexical_seal(ManifestGeneration::new(3));
        ledger.semantic_seal(ManifestGeneration::new(2));
        ledger.semantic_seal(ManifestGeneration::new(5));

        assert_eq!(ledger.lexical_sealed(), Some(ManifestGeneration::new(7)));
        assert_eq!(ledger.semantic_sealed(), Some(ManifestGeneration::new(5)));
    }

    #[test]
    fn last_seen_cursors_are_track_local() {
        let mut ledger = Ledger::default();
        ledger.set_lexical_last_seen(ChannelSeq::new(11));
        ledger.set_semantic_last_seen(ChannelSeq::new(19));

        assert_eq!(ledger.lexical_last_seen(), ChannelSeq::new(11));
        assert_eq!(ledger.semantic_last_seen(), ChannelSeq::new(19));
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts persistence/resolution via assert_eq! macros"
    )]
    fn activation_catalog_persists_and_resolves_track_locally() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let request = SearchPlaneActivateGenerationRequest {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(17),
            manifest_digest: "digest-17".to_string(),
            tracks: vec![SearchPlaneTrackKind::Lexical],
        };
        catalog.activate(&request)?;

        let pin = catalog.resolve(
            &RepoId::new("repo"),
            &RevisionId::new("rev"),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(pin.manifest_generation, ManifestGeneration::new(17));

        let reopened = ActivationCatalog::open(dir.path())?;
        let reopened_pin = reopened.resolve(
            &RepoId::new("repo"),
            &RevisionId::new("rev"),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(
            reopened_pin.manifest_generation,
            ManifestGeneration::new(17)
        );
        Ok(())
    }
}
