use super::*;
use quanta_index_contract::{
    ArtifactContentDigestV1, ArtifactIdentityV1, LogicalGenerationIdentityV1, RepoId,
    RepoMapCandidateCommitmentsV1, RepositoryRevisionIdentityV1, RevisionId,
};
use quanta_index_core::publication_admission::PublicationObjectReader;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) struct RecordingPhysical {
    root: PathBuf,
    calls: Arc<Mutex<Vec<&'static str>>>,
    refuse_read: Arc<AtomicBool>,
    refuse_candidate_open: Arc<AtomicBool>,
}

impl RecordingPhysical {
    pub(crate) fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            calls: Arc::new(Mutex::new(Vec::new())),
            refuse_read: Arc::new(AtomicBool::new(false)),
            refuse_candidate_open: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(crate) fn refuse_candidate_open(&self) {
        self.refuse_candidate_open.store(true, Ordering::SeqCst);
    }

    fn record(&self, call: &'static str) -> Result<(), CoreError> {
        self.calls
            .lock()
            .map_err(|error| CoreError::Storage(error.to_string()))?
            .push(call);
        Ok(())
    }

    fn count(&self, call: &'static str) -> Result<usize, CoreError> {
        Ok(self
            .calls
            .lock()
            .map_err(|error| CoreError::Storage(error.to_string()))?
            .iter()
            .filter(|observed| **observed == call)
            .count())
    }

    fn check_path(&self, path: &Path) -> Result<(), CoreError> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(CoreError::InvalidContract(
                "fixture physical path escaped root".into(),
            ))
        }
    }
}

struct RecordingReader {
    file: File,
    calls: Arc<Mutex<Vec<&'static str>>>,
    refuse_read: Arc<AtomicBool>,
}

impl Read for RecordingReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.refuse_read.load(Ordering::SeqCst) {
            return Err(io::Error::other(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
                message: "fixture physical reader refusal".into(),
            }));
        }
        self.file.read(bytes)
    }
}
impl Seek for RecordingReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}
impl PublicationObjectReader for RecordingReader {
    fn metadata(&self) -> Result<fs::Metadata, CoreError> {
        self.file
            .metadata()
            .map_err(|error| CoreError::storage_io("fixture reader metadata", &error))
    }
    fn sync_posix(&self) -> Result<(), CoreError> {
        self.file
            .sync_all()
            .map_err(|error| CoreError::storage_io("fixture reader sync", &error))
    }
    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        self.calls
            .lock()
            .map_err(|error| CoreError::Storage(error.to_string()))?
            .push("retire");
        drop(self.file);
        Ok(())
    }
}

impl RepoMapPhysicalFilePort for RecordingPhysical {
    fn validate_root_and_reconcile(&self, root: &Path) -> Result<(), CoreError> {
        if root != self.root {
            return Err(CoreError::InvalidContract(
                "fixture wrong RepoMap root".into(),
            ));
        }
        self.record("validate")
    }

    fn ensure_private_directory(&self, path: &Path) -> Result<(), CoreError> {
        self.check_path(path)?;
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(CoreError::storage_io(
                    "fixture create private directory",
                    &error,
                ));
            }
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| CoreError::storage_io("fixture private directory mode", &error))?;
        self.record("ensure_dir")
    }

    fn put_immutable(
        &self,
        path: &Path,
        bytes: &[u8],
        class: RepoMapPhysicalFileClass,
    ) -> Result<RepoMapImmutablePutOutcome, CoreError> {
        self.check_path(path)?;
        if (class == RepoMapPhysicalFileClass::RootUuid)
            != (path == self.root.join(ROOT_UUID_FILE_NAME))
        {
            return Err(CoreError::InvalidContract(
                "fixture class/path mismatch".into(),
            ));
        }
        if class == RepoMapPhysicalFileClass::Candidate {
            self.record("candidate_put")?;
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(mut file) => {
                file.write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .map_err(|error| CoreError::storage_io("fixture immutable write", &error))?;
                drop(file);
                let parent = path
                    .parent()
                    .ok_or_else(|| CoreError::InvalidContract("fixture no parent".into()))?;
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| CoreError::storage_io("fixture parent sync", &error))?;
                self.record("created")?;
                Ok(RepoMapImmutablePutOutcome::Created)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if fs::read(path)
                    .map_err(|read| CoreError::storage_io("fixture replay read", &read))?
                    != bytes
                {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                        message: "fixture immutable replay bytes differ".into(),
                    });
                }
                self.record("replayed")?;
                Ok(RepoMapImmutablePutOutcome::ExactReplay)
            }
            Err(error) => Err(CoreError::storage_io("fixture immutable create", &error)),
        }
    }

    fn open_reader(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError> {
        self.check_path(path)?;
        if path.starts_with(self.root.join("objects"))
            && self.refuse_candidate_open.load(Ordering::SeqCst)
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
                message: "fixture candidate reader admission refusal".into(),
            });
        }
        match File::open(path) {
            Ok(file) => {
                self.record("open_reader")?;
                Ok(Some(Box::new(RecordingReader {
                    file,
                    calls: Arc::clone(&self.calls),
                    refuse_read: Arc::clone(&self.refuse_read),
                })))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(CoreError::storage_io("fixture reader open", &error)),
        }
    }

    fn delete_file_and_sync(
        &self,
        path: &Path,
        inode: InodeKey,
        expected_length: u64,
    ) -> Result<(), CoreError> {
        self.check_path(path)?;
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| CoreError::storage_io("fixture deletion witness", &error))?;
        if (metadata.dev(), metadata.ino(), metadata.len())
            != (inode.device, inode.inode, expected_length)
        {
            return Err(CoreError::InvalidContract(
                "fixture deletion witness changed".into(),
            ));
        }
        fs::remove_file(path).map_err(|error| CoreError::storage_io("fixture delete", &error))?;
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::InvalidContract("fixture no parent".into()))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| CoreError::storage_io("fixture deletion parent sync", &error))?;
        self.record("delete")
    }
}

#[test]
fn qualified_root_refusal_precedes_uuid_or_directory_creation() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("repo-map");
    let foreign = RecordingPhysical::new(parent.path().join("foreign"));
    if RepoMapObjectStore::open_with_file_admission(&root, foreign).is_ok() || root.exists() {
        return Err("foreign authority created the RepoMap root or UUID".into());
    }
    Ok(())
}

#[test]
fn qualified_uuid_projection_replay_reader_retirement_and_witnessed_delete() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("repo-map");
    let physical = RecordingPhysical::new(root.clone());
    let store = RepoMapObjectStore::open_with_file_admission(&root, physical.clone())?;
    if fs::read(root.join(ROOT_UUID_FILE_NAME))?.len() != ROOT_UUID_BYTES
        || physical.count("created")? != 1
        || physical.count("open_reader")? != physical.count("retire")?
    {
        return Err("qualified UUID birth or original-reader retirement failed".into());
    }
    let payload = b"fixed quarantine bytes";
    let digest = QuarantinePayloadDigestV1::for_payload(payload);
    let relative = crate::layout_v3::QuarantinePayloadAddressV1::new(digest).relative_path();
    write_content_addressed(
        &root,
        &relative,
        payload,
        "quarantine payload",
        store.physical.as_ref(),
        RepoMapPhysicalFileClass::Payload,
    )?;
    write_content_addressed(
        &root,
        &relative,
        payload,
        "quarantine payload",
        store.physical.as_ref(),
        RepoMapPhysicalFileClass::Payload,
    )?;
    if store.read_raw_verified(&relative)?.as_slice() != payload
        || store.read_quarantine_payload(digest)?.as_deref() != Some(payload.as_slice())
        || physical.count("replayed")? != 1
    {
        return Err("qualified projection exact bytes or replay failed".into());
    }
    physical.refuse_read.store(true, Ordering::SeqCst);
    if !matches!(
        store.read_quarantine_payload(digest),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
            ..
        })
    ) || physical.count("open_reader")? != physical.count("retire")?
    {
        return Err("qualified read failure lost typed cause or original FD retirement".into());
    }
    physical.refuse_read.store(false, Ordering::SeqCst);
    let path = root.join(&relative);
    if !matches!(
        write_content_addressed(
            &root,
            &relative,
            b"other",
            "quarantine payload",
            store.physical.as_ref(),
            RepoMapPhysicalFileClass::Payload
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            ..
        })
    ) || fs::read(&path)?.as_slice() != payload
    {
        return Err("different-byte replay replaced or accepted an immutable projection".into());
    }
    let _reclaimed = store.reclaim_quarantine_payload(digest)?;
    if path.exists()
        || store.read_quarantine_payload(digest)?.is_some()
        || physical.count("delete")? != 1
        || physical.count("open_reader")? != physical.count("retire")?
    {
        return Err("qualified payload deletion or original-FD retirement failed".into());
    }
    Ok(())
}

#[test]
fn qualified_candidate_seal_replay_and_reclaim_keep_canonical_bytes() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("repo-map");
    let physical = RecordingPhysical::new(root.clone());
    let store = RepoMapObjectStore::open_with_file_admission(&root, physical.clone())?;
    let identity = LogicalGenerationIdentityV1::new(
        RepositoryRevisionIdentityV1::new(RepoId::new("repo")?, RevisionId::new("revision")?),
        7,
    );
    let payload = b"fixed compiled RepoMap bytes".to_vec();
    let artifact = ArtifactIdentityV1::new(
        identity.clone(),
        ArtifactContentDigestV1::for_payload(&payload),
        u64::try_from(payload.len())?,
    );
    let candidate = RepoMapCandidateEnvelopeV1::new(
        identity,
        RepoMapCandidateCommitmentsV1 {
            producer_manifest: [1; 32],
            producer_authority: [2; 32],
            compiled_graph: [3; 32],
            schema: [4; 32],
            projection_profile: [5; 32],
        },
        artifact,
        payload,
    )?;
    let expected = candidate.encode_canonical()?;
    let created = store.seal(&candidate)?;
    let replayed = store.seal(&candidate)?;
    let digest = created.address.digest();
    let address = root.join(created.address.relative_path());
    if created.replayed
        || !replayed.replayed
        || fs::read(&address)? != expected
        || !store.candidate_exists_owned(digest)?
        || physical.count("candidate_put")? != 2
    {
        return Err("qualified candidate birth, exact replay, or canonical bytes failed".into());
    }
    store.unlink_object(digest)?;
    if store.candidate_exists_owned(digest)?
        || address.exists()
        || physical.count("delete")? != 1
        || physical.count("open_reader")? != physical.count("retire")?
    {
        return Err(
            "qualified candidate reclaim lost witnessed deletion or reader retirement".into(),
        );
    }
    Ok(())
}
