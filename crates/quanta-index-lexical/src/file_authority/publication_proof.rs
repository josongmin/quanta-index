//! Publication-only membership proof custody. No source bodies are retained.

use std::collections::BTreeMap;

use quanta_index_contract::{FileCoverageSnapshot, GenerationSnapshot};
use quanta_index_core::SealedArtifactCommitmentV1;
use sha2::{Digest as _, Sha256};

use super::{ObjectIdentity, VerifiedAuthority, root};

/// Constructed only from the independent source/normalization/posting proof.
///
/// The root commitment binds every ordered source row, stable source ID,
/// language, admission, resident charge and all pack/posting descriptors.
pub(crate) struct ValidatedFileAuthorityProof {
    identity: GenerationSnapshot,
    root_commitment: SealedArtifactCommitmentV1,
    policy_sha256: [u8; 32],
}

/// Admission proof and serving materialization are different consumers.
/// A reauthenticated proof cannot be converted into a serving authority.
pub(crate) enum FileAuthorityWalkProof {
    Materialized(VerifiedAuthority),
    Validated(super::verify::VerifiedAuthority<super::verify::PublicationOutput>),
    Reauthenticated(root::AuthorityRoot),
}

impl FileAuthorityWalkProof {
    fn root(&self) -> &root::AuthorityRoot {
        match self {
            Self::Materialized(verified) => &verified.authority.root,
            Self::Validated(verified) => &verified.root,
            Self::Reauthenticated(root) => root,
        }
    }

    pub(crate) fn inventory(&self) -> BTreeMap<String, u64> {
        super::inventory_for_root(self.root())
    }

    pub(crate) fn matches_coverage(&self, coverage: &FileCoverageSnapshot) -> bool {
        super::root_matches_coverage(self.root(), coverage)
    }

    /// Only the full proof may establish new retained membership authority.
    /// A reauthenticated proof leaves the existing private witness in place.
    pub(crate) fn retain(
        &self,
        identity: &GenerationSnapshot,
        commitment: &SealedArtifactCommitmentV1,
    ) -> Option<ValidatedFileAuthorityProof> {
        match self {
            Self::Materialized(_) | Self::Validated(_) => Some(ValidatedFileAuthorityProof {
                identity: identity.clone(),
                root_commitment: commitment.clone(),
                policy_sha256: self.root().policy_sha256,
            }),
            Self::Reauthenticated(_) => None,
        }
    }
}

/// Reuse semantic membership proof only after authenticating every current
/// object. No inode/mtime/length observation stands in for its committed hash.
pub(crate) fn verify_for_publication<R>(
    root_bytes: &[u8],
    identity: &GenerationSnapshot,
    commitment: &SealedArtifactCommitmentV1,
    retained: Option<&ValidatedFileAuthorityProof>,
    mut read_blob: R,
) -> Result<FileAuthorityWalkProof, String>
where
    R: FnMut([u8; 32], u64) -> Result<(Vec<u8>, ObjectIdentity), String>,
{
    let policy = super::policy();
    let current_digest: [u8; 32] = Sha256::digest(root_bytes).into();
    if current_digest != commitment.sha256
        || u64::try_from(root_bytes.len())
            .map_err(|error| format!("publication F15 root byte width: {error}"))?
            != commitment.bytes
    {
        return Err("publication F15 root differs from committed binding".into());
    }
    let matching = retained.filter(|proof| {
        proof.identity == *identity
            && proof.root_commitment == *commitment
            && proof.policy_sha256 == policy.digest()
    });
    let Some(_proof) = matching else {
        return super::verify_v15_for_publication(root_bytes, read_blob)
            .map(FileAuthorityWalkProof::Validated);
    };
    crate::causal_profile::timed_work("lexical_file_authority_proof_reauthentication", || {
        // Decode still enforces current policy, strict ordered source keys,
        // stable IDs, descriptor inventories, lengths and aggregate admission.
        // The identical authenticated root binds the already-proved semantic
        // relation between these rows and the exact authenticated object bytes.
        let root = root::AuthorityRoot::decode(root_bytes, policy)?;
        let mut pinned = BTreeMap::new();
        for partition in root
            .packs
            .iter()
            .chain(&root.path_postings)
            .chain(&root.content_postings)
        {
            let (bytes, current) = read_blob(partition.sha256, partition.bytes)?;
            if u64::try_from(bytes.len())
                .map_err(|error| format!("publication F15 object byte width: {error}"))?
                != partition.bytes
                || current.len != partition.bytes
                || <[u8; 32]>::from(Sha256::digest(&bytes)) != partition.sha256
            {
                return Err("publication F15 object differs from committed bytes".into());
            }
            if pinned
                .insert(partition.sha256, current)
                .is_some_and(|previous| previous != current)
            {
                return Err("F15 object identity changed during publication verification".into());
            }
        }
        if pinned.len() != super::object_inventory(&root).len() {
            return Err("F15 authenticated object inventory differs from root".into());
        }
        Ok(FileAuthorityWalkProof::Reauthenticated(root))
    })
}

#[cfg(test)]
mod tests {
    use super::{FileAuthorityWalkProof, verify_for_publication};
    use crate::file_authority::{self, SourceFile, TRIGRAM_BITMAP_BYTES};
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
        SearchPlaneTrackKind, SourceFileKey, SourceFileRevision,
    };
    use quanta_index_core::SealedArtifactCommitmentV1;
    use sha2::{Digest as _, Sha256};
    use std::cell::Cell;
    use std::path::Path;

    fn fixture() -> (
        tempfile::TempDir,
        GenerationSnapshot,
        Vec<u8>,
        SealedArtifactCommitmentV1,
    ) {
        fixture_with_paths(&["src/a.rs"])
    }

    fn fixture_with_paths(
        paths: &[&str],
    ) -> (
        tempfile::TempDir,
        GenerationSnapshot,
        Vec<u8>,
        SealedArtifactCommitmentV1,
    ) {
        let dir = tempfile::tempdir().expect("fixture generation");
        let raw = b"proofmarker";
        let mut files = Vec::new();
        for path in paths {
            let source = SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source").expect("source repo"),
                    repo_relative_path: RepoRelativePath::new(*path),
                },
                revision_id: RevisionId::new("source-revision").expect("source revision"),
                source_sha256: Sha256::digest(raw).into(),
            };
            let expected_postings = file_authority::source_posting_memberships(
                &source,
                raw,
                true,
                &mut vec![0; TRIGRAM_BITMAP_BYTES],
            )
            .expect("fixture memberships");
            files.push(SourceFile {
                source,
                bytes: raw.to_vec().into(),
                text_admitted: true,
                language: LanguageCode::new("rust").expect("fixture language"),
                indexed_text: None,
                folded_text: None,
                indexed_path: String::new(),
                folded_path: String::new(),
                expected_postings,
            });
        }
        let _authority = file_authority::from_test_files(files, dir.path())
            .expect("independent packed source proof");
        let bytes = std::fs::read(file_authority::root_path(dir.path())).expect("root bytes");
        let commitment = SealedArtifactCommitmentV1 {
            name: format!("{}/{}", file_authority::DIR, file_authority::ROOT),
            bytes: u64::try_from(bytes.len()).expect("root width"),
            sha256: Sha256::digest(&bytes).into(),
        };
        let identity = GenerationSnapshot {
            repo_id: RepoId::new("serving").expect("serving repo"),
            revision_id: RevisionId::new("serving-revision").expect("serving revision"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "manifest:fixture".into(),
        };
        (dir, identity, bytes, commitment)
    }

    fn read_blob(
        dir: &Path,
        digest: [u8; 32],
        len: u64,
    ) -> Result<(Vec<u8>, file_authority::ObjectIdentity), String> {
        let canonical = dir.canonicalize().map_err(|error| error.to_string())?;
        file_authority::read_object_pinned(
            &canonical
                .join(file_authority::DIR)
                .join(file_authority::OBJECTS),
            digest,
            len,
            None,
        )
        .map_err(|error| error.to_string())
    }

    #[test]
    fn retained_membership_proof_still_reads_every_current_object() {
        let (dir, identity, bytes, commitment) = fixture();
        let first = verify_for_publication(&bytes, &identity, &commitment, None, |digest, len| {
            read_blob(dir.path(), digest, len)
        })
        .expect("full proof");
        assert!(matches!(&first, FileAuthorityWalkProof::Validated(_)));
        let retained = first
            .retain(&identity, &commitment)
            .expect("validated witness");
        drop(first); // Source bodies and normalized surfaces are not retained.
        let reads = Cell::new(0_u64);
        let current = verify_for_publication(
            &bytes,
            &identity,
            &commitment,
            Some(&retained),
            |digest, len| {
                reads.set(reads.get() + 1);
                read_blob(dir.path(), digest, len)
            },
        )
        .expect("reauthenticated proof");
        assert!(matches!(
            current,
            FileAuthorityWalkProof::Reauthenticated(_)
        ));
        // One source pack, one path block and one content block in this fixture.
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn cold_publication_and_serving_refuse_changed_shared_object_identity() {
        let (dir, _identity, bytes, _commitment) = fixture_with_paths(&["src/a.rs", "src/b.rs"]);
        let root = file_authority::root::AuthorityRoot::decode(&bytes, file_authority::policy())
            .expect("root");
        assert_eq!(root.packs.len(), 2);
        assert_eq!(
            root.packs.first().expect("first pack").sha256,
            root.packs.last().expect("last pack").sha256
        );
        for publication in [false, true] {
            let mut seen = std::collections::BTreeSet::new();
            let read = |digest, len| {
                let (bytes, mut pinned) = read_blob(dir.path(), digest, len)?;
                if !seen.insert(digest) {
                    pinned.ino = pinned.ino.checked_add(1).expect("fixture inode width");
                }
                Ok((bytes, pinned))
            };
            let refusal = if publication {
                file_authority::verify_v15_for_publication(&bytes, read).map(|_verified| ())
            } else {
                file_authority::verify_v15(&bytes, read).map(|_verified| ())
            }
            .expect_err("one committed digest changed physical identity during the walk");
            assert_eq!(
                refusal,
                "F15 object identity changed during cold verification"
            );
        }
    }

    #[test]
    fn current_same_size_tamper_refuses_retained_and_independent_cold_proofs() {
        let (dir, identity, bytes, commitment) = fixture();
        let first = verify_for_publication(&bytes, &identity, &commitment, None, |digest, len| {
            read_blob(dir.path(), digest, len)
        })
        .expect("full proof");
        let retained = first
            .retain(&identity, &commitment)
            .expect("validated witness");
        let root = file_authority::root::AuthorityRoot::decode(&bytes, file_authority::policy())
            .expect("root");
        for descriptor in root
            .packs
            .iter()
            .chain(&root.path_postings)
            .chain(&root.content_postings)
        {
            let path = dir
                .path()
                .join(file_authority::object_name(&descriptor.sha256));
            let original = std::fs::read(&path).expect("object bytes");
            let modified = std::fs::metadata(&path)
                .expect("object metadata")
                .modified()
                .expect("object mtime");
            let mut changed = original.clone();
            *changed.last_mut().expect("nonempty object") ^= 1;
            std::fs::write(&path, &changed).expect("in-place same-size tamper");
            std::fs::File::open(&path)
                .expect("tampered object")
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .expect("restore original mtime");
            assert!(
                verify_for_publication(
                    &bytes,
                    &identity,
                    &commitment,
                    Some(&retained),
                    |digest, len| read_blob(dir.path(), digest, len)
                )
                .is_err()
            );
            assert!(
                file_authority::verify_v15(&bytes, |digest, len| read_blob(
                    dir.path(),
                    digest,
                    len
                ))
                .is_err()
            );
            std::fs::write(path, original).expect("restore object");
        }
    }

    #[test]
    fn identity_manifest_policy_and_ordered_metadata_cannot_borrow_another_proof() {
        let (dir, identity, bytes, commitment) = fixture();
        let first = verify_for_publication(&bytes, &identity, &commitment, None, |digest, len| {
            read_blob(dir.path(), digest, len)
        })
        .expect("full proof");
        let mut retained = first
            .retain(&identity, &commitment)
            .expect("validated witness");
        let mut other_generation = identity.clone();
        other_generation.manifest_generation = ManifestGeneration::new(2);
        let mut other_manifest = identity.clone();
        other_manifest.manifest_digest = "manifest:other".into();
        for changed in [other_generation, other_manifest] {
            let observed = verify_for_publication(
                &bytes,
                &changed,
                &commitment,
                Some(&retained),
                |digest, len| read_blob(dir.path(), digest, len),
            )
            .expect("new full proof");
            assert!(matches!(observed, FileAuthorityWalkProof::Validated(_)));
        }
        retained.policy_sha256 = [0; 32];
        let observed = verify_for_publication(
            &bytes,
            &identity,
            &commitment,
            Some(&retained),
            |digest, len| read_blob(dir.path(), digest, len),
        )
        .expect("current policy full proof");
        assert!(matches!(observed, FileAuthorityWalkProof::Validated(_)));
        let retained = first
            .retain(&identity, &commitment)
            .expect("original validated witness");
        let mut root =
            file_authority::root::AuthorityRoot::decode(&bytes, file_authority::policy())
                .expect("root");
        // Equal-length language metadata preserves the independent resident
        // charge, but it is a different ordered source-row commitment.
        root.sources.first_mut().expect("one source").language =
            LanguageCode::new("ruby").expect("language");
        let changed = root
            .encode(file_authority::policy())
            .expect("changed canonical root");
        let changed_commitment = SealedArtifactCommitmentV1 {
            name: commitment.name.clone(),
            bytes: u64::try_from(changed.len()).expect("root width"),
            sha256: Sha256::digest(&changed).into(),
        };
        let observed = verify_for_publication(
            &changed,
            &identity,
            &changed_commitment,
            Some(&retained),
            |digest, len| read_blob(dir.path(), digest, len),
        )
        .expect("new metadata full proof");
        assert!(matches!(observed, FileAuthorityWalkProof::Validated(_)));
        assert!(
            verify_for_publication(
                &changed,
                &identity,
                &commitment,
                Some(&retained),
                |digest, len| read_blob(dir.path(), digest, len)
            )
            .is_err(),
            "stale root commitment admitted changed metadata"
        );
    }
}
