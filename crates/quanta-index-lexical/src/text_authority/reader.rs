//! Generation-bound text authority with transient shard decoding.
//!
//! Cold open proves every shard. Serving retains only the authenticated
//! shard entries, file descriptors and compact document identities. A query
//! decodes one shard at a time; keyword queries need no shard reads.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};

use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_positions::ShardedPositionsIndex;
use quanta_index_lq_trigram::{DocId as TrigramDocId, DocResolver, ShardedTrigramIndex};

use crate::text_authority::manifest::{ShardEntry, TEXT_AUTHORITY_DIR_NAME};
use crate::text_authority::shard::{ShardBody, TextAuthorityDoc, sha256_of_bytes};

use quanta_index_core::count_from_usize as count_usize;

pub(crate) struct ProvedTextShard {
    pub(crate) entry: ShardEntry,
    pub(crate) file: File,
}

enum ShardStorage {
    Paged {
        _root: File,
        directory: PathBuf,
        shards: Vec<ProvedTextShard>,
    },
    #[cfg(test)]
    Resident(Vec<ShardBody>),
}

/// Authenticated immutable shard descriptors; decoded bodies are request scratch.
pub(crate) struct ShardedTextAuthority {
    storage: ShardStorage,
    documents: BTreeMap<u64, TextDocIdentity>,
}

/// Small identity commitment used by selected-row integrity checks.
pub(crate) struct TextDocIdentity {
    pub(crate) candidate_id: String,
    pub(crate) indexed_sha256: [u8; 32],
}

pub(crate) fn document_identities(body: &ShardBody) -> BTreeMap<u64, TextDocIdentity> {
    body.docs_by_id
        .iter()
        .map(|(id, doc)| {
            (
                *id,
                TextDocIdentity {
                    candidate_id: doc.candidate_id.clone(),
                    indexed_sha256: sha256_of_bytes(doc.indexed_text.as_bytes()),
                },
            )
        })
        .collect()
}

impl ShardedTextAuthority {
    pub(crate) fn from_proved_files(
        root: &File,
        directory: &Path,
        shards: Vec<ProvedTextShard>,
        documents: BTreeMap<u64, TextDocIdentity>,
    ) -> Result<Self, CoreError> {
        let root = root.try_clone().map_err(|error| {
            CoreError::Storage(format!("lexical: pin proved text authority: {error}"))
        })?;
        Ok(Self {
            documents,
            storage: ShardStorage::Paged {
                _root: root,
                directory: directory.into(),
                shards,
            },
        })
    }

    #[cfg(test)]
    pub(crate) fn from_proved_shards(shards: Vec<(u64, ShardBody)>) -> Result<Self, CoreError> {
        if shards
            .windows(2)
            .any(|pair| matches!(pair, [(previous, _), (next, _) ] if previous >= next))
        {
            return Err(CoreError::InvalidContract(
                "lexical: text shards out of ascending order".into(),
            ));
        }
        let documents = shards
            .iter()
            .flat_map(|(_, body)| document_identities(body))
            .collect();
        Ok(Self {
            documents,
            storage: ShardStorage::Resident(shards.into_iter().map(|(_, body)| body).collect()),
        })
    }

    pub(crate) fn heap_bytes_estimate(&self) -> u64 {
        let storage = match &self.storage {
            ShardStorage::Paged {
                directory, shards, ..
            } => count_usize(directory.as_os_str().len()).saturating_add(
                count_usize(shards.capacity())
                    .saturating_mul(count_usize(std::mem::size_of::<ProvedTextShard>())),
            ),
            #[cfg(test)]
            ShardStorage::Resident(shards) => shards.iter().fold(0_u64, |total, shard| {
                total.saturating_add(shard.heap_bytes_estimate())
            }),
        };
        self.documents.values().fold(storage, |total, doc| {
            total
                .saturating_add(128)
                .saturating_add(count_usize(doc.candidate_id.capacity()))
        })
    }

    /// Cold open committed these identities from proved shard bodies.
    pub(crate) fn matches_document(&self, id: u64, candidate_id: &str, indexed: &str) -> bool {
        self.documents.get(&id).is_some_and(|doc| {
            doc.candidate_id == candidate_id
                && doc.indexed_sha256 == sha256_of_bytes(indexed.as_bytes())
        })
    }

    /// A body never escapes this callback or becomes cached snapshot heap.
    /// Every read rechecks the pinned inode's committed digest and shape.
    /// Existing handles survive directory reclaim and ignore pathname replacement.
    pub(crate) fn visit_shards(
        &self,
        budget: &RequestBudgetV1,
        mut visit: impl FnMut(TextAuthorityShard<'_>) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        match &self.storage {
            ShardStorage::Paged {
                directory, shards, ..
            } => {
                if shards.is_empty() {
                    visit(TextAuthorityShard { body: None })?;
                }
                for shard in shards {
                    budget.checkpoint("lexical:text-shard-read")?;
                    let body =
                        decode_opened_shard(&shard.file, directory, &shard.entry, Some(budget))?;
                    budget.checkpoint("lexical:text-shard-decode")?;
                    visit(TextAuthorityShard { body: Some(&body) })?;
                }
            }
            #[cfg(test)]
            ShardStorage::Resident(shards) => {
                if shards.is_empty() {
                    visit(TextAuthorityShard { body: None })?;
                }
                for body in shards {
                    budget.checkpoint("lexical:text-shard-read")?;
                    visit(TextAuthorityShard { body: Some(body) })?;
                }
            }
        }
        budget.checkpoint("lexical:text-shards-complete")
    }
}

pub(crate) struct TextAuthorityShard<'a> {
    body: Option<&'a ShardBody>,
}

impl TextAuthorityShard<'_> {
    pub(crate) fn doc(&self, doc_id: u64) -> Option<&TextAuthorityDoc> {
        self.body.and_then(|body| body.docs_by_id.get(&doc_id))
    }
    pub(crate) fn doc_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.body
            .into_iter()
            .flat_map(|body| body.docs_by_id.keys().copied())
    }
    pub(crate) fn trigram_index(&self, folded: bool) -> ShardedTrigramIndex<'_> {
        ShardedTrigramIndex::new(
            self.body
                .into_iter()
                .map(|body| {
                    if folded {
                        &body.trigram_folded
                    } else {
                        &body.trigram
                    }
                })
                .collect(),
        )
    }
    pub(crate) fn positions_index(&self, case_sensitive: bool) -> ShardedPositionsIndex<'_> {
        ShardedPositionsIndex::new(
            self.body
                .into_iter()
                .map(|body| {
                    if case_sensitive {
                        &body.positions
                    } else {
                        &body.positions_folded
                    }
                })
                .collect(),
        )
    }
    pub(crate) fn resolver(&self, folded: bool) -> TextAuthorityResolver<'_> {
        TextAuthorityResolver {
            body: self.body,
            folded,
        }
    }
}

pub(crate) struct TextAuthorityResolver<'a> {
    body: Option<&'a ShardBody>,
    folded: bool,
}
impl DocResolver for TextAuthorityResolver<'_> {
    fn resolve(&self, doc_id: TrigramDocId) -> Option<&[u8]> {
        let doc = self.body?.docs_by_id.get(&doc_id.0)?;
        Some(if self.folded {
            doc.folded_indexed_text.as_bytes()
        } else {
            doc.indexed_text.as_bytes()
        })
    }
}

/// Read, prove and decode one listed shard.
///
/// Shared by the sealed-generation walk (every shard, for both doors) and
/// the incremental writer (the touched shards only), so every reader
/// proves the same commitment.
pub(crate) fn load_shard(
    generation_dir: &Path,
    entry: &ShardEntry,
) -> Result<ShardBody, CoreError> {
    load_shard_with(generation_dir, entry, None, |name| {
        crate::sealed_generation::open_regular_nofollow(generation_dir, name)
    })
    .map(|(body, _file)| body)
}

pub(crate) fn load_shard_file_at(
    root: &File,
    generation_dir: &Path,
    entry: &ShardEntry,
    budget: Option<&RequestBudgetV1>,
) -> Result<(ShardBody, File), CoreError> {
    load_shard_with(generation_dir, entry, budget, |name| {
        crate::sealed_generation::open_regular_below(root, name)
    })
}

fn load_shard_with(
    generation_dir: &Path,
    entry: &ShardEntry,
    budget: Option<&RequestBudgetV1>,
    open: impl FnOnce(&Path) -> std::io::Result<File>,
) -> Result<(ShardBody, File), CoreError> {
    let path = entry.path(generation_dir);
    let name = format!("{TEXT_AUTHORITY_DIR_NAME}/{}", entry.file_name());
    let file = open(Path::new(&name)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            crate::index_store::sidecar_corrupt(generation_dir, &name, "missing")
        } else if crate::sealed_generation::is_unsafe_artifact_path(&error) {
            crate::index_store::sidecar_corrupt(generation_dir, &name, "not a regular sealed file")
        } else {
            CoreError::Storage(format!(
                "lexical: open text authority shard {}: {error}",
                path.display()
            ))
        }
    })?;
    let body = decode_opened_shard(&file, generation_dir, entry, budget)?;
    Ok((body, file))
}

/// Positioned reads let concurrent queries share authenticated file descriptors.
fn decode_opened_shard(
    file: &File,
    generation_dir: &Path,
    entry: &ShardEntry,
    budget: Option<&RequestBudgetV1>,
) -> Result<ShardBody, CoreError> {
    let path = entry.path(generation_dir);
    let name = format!("{TEXT_AUTHORITY_DIR_NAME}/{}", entry.file_name());
    let opened_len = file
        .metadata()
        .map_err(|error| {
            CoreError::Storage(format!("lexical: stat shard {}: {error}", path.display()))
        })?
        .len();
    if opened_len != entry.bytes {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &name,
            "opened shard differs from committed length",
        ));
    }
    let admitted = usize::try_from(entry.bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: committed shard length overflows usize: {error}"
        ))
    })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(admitted).map_err(|error| {
        CoreError::Storage(format!("lexical: allocate admitted text shard: {error}"))
    })?;
    bytes.resize(admitted, 0);
    let mut offset = 0_u64;
    for chunk in bytes.chunks_mut(64 * 1024) {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:text-shard-bytes")?;
        }
        file.read_exact_at(chunk, offset).map_err(|error| {
            if matches!(
                error.kind(),
                ErrorKind::UnexpectedEof | ErrorKind::InvalidData
            ) {
                crate::index_store::sidecar_corrupt(
                    generation_dir,
                    &name,
                    &format!("read admitted shard: {error}"),
                )
            } else {
                CoreError::Storage(format!("lexical: read shard {}: {error}", path.display()))
            }
        })?;
        offset = offset
            .checked_add(count_usize(chunk.len()))
            .ok_or_else(|| CoreError::Storage("lexical: shard read offset overflow".into()))?;
    }
    if file
        .read_at(&mut [0_u8; 1], entry.bytes)
        .map_err(|error| CoreError::Storage(format!("lexical: read shard tail: {error}")))?
        != 0
    {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &name,
            "shard grew during read",
        ));
    }
    if let Some(budget) = budget {
        budget.checkpoint("lexical:text-shard-proof")?;
    }
    let length = u64::try_from(bytes.len()).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: text authority shard {} length: {err}",
            path.display()
        ))
    })?;
    if length != entry.bytes {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &name,
            &format!("{length} bytes on disk, {} committed", entry.bytes),
        ));
    }
    if sha256_of_bytes(&bytes) != entry.sha256 {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &name,
            "content digest differs from the committed digest",
        ));
    }
    let body = ShardBody::decode(&bytes, entry.index, generation_dir, &name)?;
    let rows = body.rows()?;
    if rows != entry.rows {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &name,
            &format!("{rows} rows decoded, {} committed", entry.rows),
        ));
    }
    if body.doc_id_extremes() != Some((entry.min_doc_id, entry.max_doc_id)) {
        return Err(crate::index_store::sidecar_corrupt(
            generation_dir,
            &name,
            &format!(
                "doc ids {:?} decoded, {}..={} committed",
                body.doc_id_extremes(),
                entry.min_doc_id,
                entry.max_doc_id
            ),
        ));
    }
    Ok(body)
}

#[cfg(test)]
mod path_tests {
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_core::CoreError;

    use super::load_shard;
    use crate::text_authority::manifest::{ShardEntry, text_authority_dir};

    fn paged_fixture() -> Result<
        (
            crate::test_support::GenerationFixture,
            super::ShardedTextAuthority,
            Vec<ShardEntry>,
        ),
        Box<dyn std::error::Error>,
    > {
        let generation = crate::test_support::generation_fixture()?;
        let docs = [
            (1, "alpha beta"),
            (2048, "gamma delta"),
            (4097, "alpha gamma"),
        ]
        .into_iter()
        .map(|(doc_id, text)| crate::text_authority::AddedTextDoc {
            doc_id,
            candidate_id: format!("candidate-{doc_id}"),
            text: text.into(),
        })
        .collect();
        let _written = crate::text_authority::rebuild(
            generation.path(),
            quanta_index_contract::ManifestGeneration::new(1),
            docs,
            None,
            4097,
        )?;
        let manifest =
            crate::text_authority::read_manifest(generation.path())?.ok_or("missing manifest")?;
        // Use the same proof path that supplies the open visitor.
        let root = crate::sealed_generation::open_generation_dir_nofollow(generation.path())?;
        let mut documents = std::collections::BTreeMap::new();
        let mut proved = Vec::new();
        for entry in &manifest.shards {
            let (body, file) = super::load_shard_file_at(&root, generation.path(), entry, None)?;
            documents.extend(super::document_identities(&body));
            proved.push(super::ProvedTextShard {
                entry: entry.clone(),
                file,
            });
        }
        let authority = super::ShardedTextAuthority::from_proved_files(
            &root,
            generation.path(),
            proved,
            documents,
        )?;
        Ok((generation, authority, manifest.shards))
    }

    fn budget() -> quanta_index_core::RequestBudgetV1 {
        quanta_index_core::RequestBudgetV1::unbounded()
    }

    fn substring_ids(authority: &super::ShardedTextAuthority) -> Result<Vec<u64>, CoreError> {
        let mut ids = Vec::new();
        authority.visit_shards(&budget(), |shard| {
            let found = quanta_index_lq_trigram::query_raw_substring(
                &shard.trigram_index(false),
                b"alpha",
                &shard.resolver(false),
            )
            .map_err(|error| CoreError::Storage(error.to_string()))?;
            ids.extend(found.into_iter().map(|id| id.0));
            Ok(())
        })?;
        Ok(ids)
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "independent fixed regression assertions"
    )]
    fn paged_text_authority_keeps_descriptors_and_fixed_cross_shard_answers()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_generation, authority, entries) = paged_fixture()?;
        assert_eq!(entries.len(), 3);
        assert!(authority.matches_document(1, "candidate-1", "alpha beta"));
        assert!(!authority.matches_document(1, "other-candidate", "alpha beta"));
        assert!(!authority.matches_document(1, "candidate-1", "alpha gamma"));
        assert!(!authority.matches_document(2, "candidate-1", "alpha beta"));
        assert!(
            authority.heap_bytes_estimate() < 4096,
            "decoded text postings remained resident"
        );
        assert_eq!(substring_ids(&authority)?, vec![1, 4097]);
        let mut phrase_ids = Vec::new();
        authority.visit_shards(&budget(), |shard| {
            let found = quanta_index_lq_positions::query_phrase(
                &shard.positions_index(true),
                &["alpha", "beta"],
            )
            .map_err(|error| CoreError::Storage(error.to_string()))?;
            phrase_ids.extend(found.matches.into_iter().map(|found| found.doc_id.0));
            Ok(())
        })?;
        assert_eq!(phrase_ids, vec![1]);
        assert_eq!(substring_ids(&authority)?, vec![1, 4097]);
        assert!(
            authority.heap_bytes_estimate() < 4096,
            "query cached a decoded shard"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "independent fixed regression assertions"
    )]
    fn paged_text_authority_is_anchored_after_generation_path_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let (generation, authority, _) = paged_fixture()?;
        let moved = generation.path().with_file_name("old-g1");
        std::fs::rename(generation.path(), &moved)?;
        std::fs::create_dir(generation.path())?;
        std::fs::create_dir(generation.path().join("text-authority"))?;
        assert_eq!(substring_ids(&authority)?, vec![1, 4097]);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "independent fixed regression assertions"
    )]
    fn paged_text_authority_refuses_inode_corruption_and_survives_path_redirect()
    -> Result<(), Box<dyn std::error::Error>> {
        for redirect in [false, true] {
            let (generation, authority, entries) = paged_fixture()?;
            let entry = entries.first().ok_or("missing shard")?;
            let path = entry.path(generation.path());
            let original = std::fs::read(&path)?;
            if redirect {
                let outside = generation.track_path().join("outside");
                std::fs::write(&outside, original)?;
                std::fs::remove_file(&path)?;
                std::os::unix::fs::symlink(&outside, &path)?;
            } else {
                std::fs::write(&path, vec![0; original.len()])?;
            }
            if redirect {
                assert_eq!(substring_ids(&authority)?, vec![1, 4097]);
            } else {
                assert!(matches!(
                    substring_ids(&authority),
                    Err(CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                        ..
                    })
                ));
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "independent fixed regression assertions"
    )]
    fn paged_text_authority_checks_cancel_before_reading_another_shard()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_generation, authority, _) = paged_fixture()?;
        let budget = budget();
        let mut visits = 0;
        let result = authority.visit_shards(&budget, |_shard| {
            visits += 1;
            budget.cancel_handle().cancel();
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(visits, 1);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "independent fixed regression assertions"
    )]
    fn pinned_text_shards_survive_unlink_and_concurrent_positioned_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let (generation, authority, _) = paged_fixture()?;
        std::fs::remove_dir_all(generation.path())?;
        assert_eq!(substring_ids(&authority)?, vec![1, 4097]);
        std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
            let workers = (0..8)
                .map(|_| scope.spawn(|| substring_ids(&authority)))
                .collect::<Vec<_>>();
            for worker in workers {
                let actual = worker
                    .join()
                    .map_err(|_panic| "positioned-read worker panicked")??;
                if actual != vec![1, 4097] {
                    return Err("concurrent positioned read changed fixed source answers".into());
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn shard_read_refuses_redirect_before_loading_bytes() -> Result<(), Box<dyn std::error::Error>>
    {
        let generation = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let directory = text_authority_dir(generation.path());
        std::fs::create_dir(&directory)?;
        let entry = ShardEntry {
            index: 0,
            rows: 1,
            min_doc_id: 1,
            max_doc_id: 1,
            bytes: 4,
            sha256: [0; 32],
        };
        let target = outside.path().join("shard");
        std::fs::write(&target, b"data")?;
        std::os::unix::fs::symlink(&target, entry.path(generation.path()))?;
        let result = load_shard(generation.path(), &entry);
        if !matches!(
            result,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ) {
            return Err("redirected shard was not refused typed".into());
        }
        Ok(())
    }
}
