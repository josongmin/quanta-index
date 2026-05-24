//! Materialize-before-activate orchestrator (D17).
//!
//! Owns the read+verify path for producer-published bundle artifact bytes:
//! search-plane never retains those bytes (no artifact-store crate), so this
//! module is the single place that touches the filesystem for bundle inputs.
//!
//! Flow per [`MaterializeUseCase::materialize`]:
//! 1. Validate the manifest's required artifact refs.
//! 2. Inline `std::fs::read` + SHA-256 verify each required and present
//!    optional artifact payload, with path-traversal guard.
//! 3. Run lexical + semantic builds **in parallel** via `std::thread::scope`.
//! 4. Only on full build success: record manifest catalog row +
//!    mark generation active. Any build failure leaves the previously active
//!    generation untouched (H-SP1).

use std::path::{Component, Path, PathBuf};

use quanta_index_contract::{
    BundleArtifactRef, PublishedGenerationSet, PublishedSearchBundleManifest,
};
use quanta_index_core::{
    BundlePolicy, CoreError, LexicalBuildInput, PublishedSearchActivationStatePort,
    PublishedSearchGenerationCatalogPort, SearchPlaneLexicalIndexBuildPort,
    SearchPlaneSemanticIndexBuildPort, SemanticBuildInput,
};
use sha2::{Digest, Sha256};

/// Outcome of a successful materialize call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterializeOutcome {
    /// Builds succeeded; manifest recorded; generation marked active.
    Activated,
}

/// Orchestrator wiring catalog/activation ports with build ports.
///
/// `control` is `&mut` because the catalog/activation ports still require
/// exclusive access (rusqlite transactions). `lexical` / `semantic` are
/// borrowed `&` because the build ports now take `&self` (D17 cleanup) —
/// adapters can therefore be shared via `Arc<T>` with the query engine in
/// the same composition root, without wrapping them in `Mutex<T>`.
pub struct MaterializeUseCase<'a, C, L, S>
where
    C: PublishedSearchGenerationCatalogPort + PublishedSearchActivationStatePort + Send,
    L: SearchPlaneLexicalIndexBuildPort + Sync,
    S: SearchPlaneSemanticIndexBuildPort + Sync,
{
    pub control: &'a mut C,
    pub lexical: &'a L,
    pub semantic: &'a S,
    pub bundle_root: PathBuf,
    pub now_ms: u64,
}

impl<C, L, S> MaterializeUseCase<'_, C, L, S>
where
    C: PublishedSearchGenerationCatalogPort + PublishedSearchActivationStatePort + Send,
    L: SearchPlaneLexicalIndexBuildPort + Sync,
    S: SearchPlaneSemanticIndexBuildPort + Sync,
{
    /// Materialize indexes for `manifest`, activate `generation` on success.
    ///
    /// `generation` carries per-component generation IDs that the manifest
    /// does not encode (lexical/symbol/semantic/etc.). Callers source it from
    /// the producer's prepare/activate request stream. `manifest` is taken by
    /// value so it can be moved into `record_generation_manifest` after the
    /// build succeeds — that's strictly cheaper than the suggested
    /// pass-by-reference + internal clone alternative.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "generation is borrowed multiple times below; the by-value signature mirrors manifest and keeps the call-site uniform"
    )]
    pub fn materialize(
        &mut self,
        manifest: PublishedSearchBundleManifest,
        generation: PublishedGenerationSet,
    ) -> Result<MaterializeOutcome, CoreError> {
        Self::validate_consistency(&manifest, &generation)?;
        Self::validate_manifest_shape(&manifest)?;

        let chunk_rows = load_and_verify_artifact(
            &self.bundle_root,
            "manifest.lexical_chunk_rows",
            &manifest.lexical_chunk_rows,
        )?;
        let symbol_rows = load_and_verify_artifact(
            &self.bundle_root,
            "manifest.symbol_rows",
            &manifest.symbol_rows,
        )?;
        let embedding_records = match manifest.embedding_records.as_ref() {
            Some(reference) => Some(load_and_verify_artifact(
                &self.bundle_root,
                "manifest.embedding_records",
                reference,
            )?),
            None => None,
        };

        let lexical_input = LexicalBuildInput {
            chunk_rows: chunk_rows.as_slice(),
            symbol_rows: symbol_rows.as_slice(),
        };
        let semantic_input = SemanticBuildInput {
            embedding_records: embedding_records.as_deref(),
        };

        let lexical = self.lexical;
        let semantic = self.semantic;
        let manifest_ref = &manifest;
        std::thread::scope(|scope| -> Result<(), CoreError> {
            let lexical_handle =
                scope.spawn(move || lexical.build_lexical_index(manifest_ref, lexical_input));
            let semantic_handle =
                scope.spawn(move || semantic.build_semantic_index(manifest_ref, semantic_input));
            let lexical_outcome = lexical_handle.join().map_err(|payload| {
                CoreError::Storage(format!(
                    "lexical build thread panicked: {}",
                    panic_message(&payload),
                ))
            })?;
            let semantic_outcome = semantic_handle.join().map_err(|payload| {
                CoreError::Storage(format!(
                    "semantic build thread panicked: {}",
                    panic_message(&payload),
                ))
            })?;
            lexical_outcome?;
            semantic_outcome?;
            Ok(())
        })?;

        // Builds proven: record manifest catalog row, then mark generation
        // active. Order matters — catalog row must exist before the activation
        // pointer references it (`mark_active_generation` upserts catalog too,
        // but recording the canonical manifest first lets inspect_bundle work
        // immediately after activation).
        self.control.record_generation_manifest(manifest)?;
        self.control
            .mark_active_generation(&generation, self.now_ms)?;
        Ok(MaterializeOutcome::Activated)
    }

    fn validate_consistency(
        manifest: &PublishedSearchBundleManifest,
        generation: &PublishedGenerationSet,
    ) -> Result<(), CoreError> {
        if manifest.repo_id != generation.repo_id {
            return Err(CoreError::InvalidContract(
                "manifest.repo_id != generation.repo_id".into(),
            ));
        }
        if manifest.revision_id != generation.revision_id {
            return Err(CoreError::InvalidContract(
                "manifest.revision_id != generation.revision_id".into(),
            ));
        }
        if manifest.manifest_generation != generation.manifest_generation {
            return Err(CoreError::InvalidContract(
                "manifest.manifest_generation != generation.manifest_generation".into(),
            ));
        }
        Ok(())
    }

    fn validate_manifest_shape(manifest: &PublishedSearchBundleManifest) -> Result<(), CoreError> {
        BundlePolicy::validate_artifact_ref(
            "manifest.lexical_chunk_rows",
            &manifest.lexical_chunk_rows,
        )?;
        BundlePolicy::validate_artifact_ref("manifest.symbol_rows", &manifest.symbol_rows)?;
        if let Some(reference) = manifest.metadata_rows.as_ref() {
            BundlePolicy::validate_artifact_ref("manifest.metadata_rows", reference)?;
        }
        if let Some(reference) = manifest.graph_rows.as_ref() {
            BundlePolicy::validate_artifact_ref("manifest.graph_rows", reference)?;
        }
        if let Some(reference) = manifest.embedding_input_views.as_ref() {
            BundlePolicy::validate_artifact_ref("manifest.embedding_input_views", reference)?;
        }
        if let Some(reference) = manifest.embedding_records.as_ref() {
            BundlePolicy::validate_artifact_ref("manifest.embedding_records", reference)?;
        }
        Ok(())
    }
}

/// Read a producer-published artifact file and verify its declared digest.
///
/// Path-traversal guard rejects absolute paths and any `..` components before
/// touching the filesystem. The verified bytes are returned by value; callers
/// do not cache or persist them (D17: search-plane = `read+verify+forget`).
pub fn load_and_verify_artifact(
    bundle_root: &Path,
    context: &str,
    reference: &BundleArtifactRef,
) -> Result<Vec<u8>, CoreError> {
    let relative = Path::new(reference.relative_path.as_str());
    for component in relative.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(CoreError::InvalidContract(format!(
                    "{context}.relative_path contains '..' component"
                )));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(CoreError::InvalidContract(format!(
                    "{context}.relative_path must be relative"
                )));
            }
        }
    }
    let full = bundle_root.join(relative);
    let bytes = std::fs::read(&full).map_err(|error| {
        CoreError::Storage(format!(
            "{context}: read {} failed: {error}",
            full.display()
        ))
    })?;
    let actual_len = u64::try_from(bytes.len())
        .map_err(|error| CoreError::Storage(format!("{context}: byte length overflow: {error}")))?;
    if actual_len != reference.byte_length {
        return Err(CoreError::InvalidContract(format!(
            "{context}: byte_length mismatch (declared {}, actual {actual_len})",
            reference.byte_length
        )));
    }
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest_hex = hex_lowercase(&hasher.finalize());
    if digest_hex.as_str() != reference.content_digest.as_str() {
        return Err(CoreError::InvalidContract(format!(
            "{context}: content_digest mismatch"
        )));
    }
    Ok(bytes)
}

/// Render a `Box<dyn Any>` panic payload as a best-effort string for error
/// context. Joiner-side helper for [`MaterializeUseCase::materialize`].
fn panic_message(payload: &Box<dyn std::any::Any + Send + 'static>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|slice| (*slice).to_owned())
        })
        .unwrap_or_else(|| "<non-string panic payload>".to_owned())
}

fn hex_lowercase(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let hi = usize::from(byte >> 4);
        let lo = usize::from(byte & 0x0f);
        let hi_char = HEX.get(hi).copied().unwrap_or(b'0');
        let lo_char = HEX.get(lo).copied().unwrap_or(b'0');
        out.push(char::from(hi_char));
        out.push(char::from(lo_char));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use quanta_index_contract::{
        BundleArtifactRef, BundleEncoding, GenerationId, ManifestDigest, ManifestGeneration,
        PublishedGenerationSet, PublishedSearchBundleManifest, RepoId, RevisionId,
    };
    use quanta_index_core::{
        CoreError, LexicalBuildInput, PublishedSearchActivationStatePort,
        PublishedSearchGenerationCatalogPort, SearchPlaneLexicalIndexBuildPort,
        SearchPlaneSemanticIndexBuildPort, SemanticBuildInput,
    };
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{MaterializeOutcome, MaterializeUseCase, hex_lowercase, load_and_verify_artifact};

    macro_rules! ok_or_fail {
        ($expr:expr, $msg:expr) => {
            match $expr {
                Ok(v) => v,
                Err(error) => {
                    assert!(false, "{}: {error}", $msg);
                    return;
                }
            }
        };
    }

    #[derive(Default)]
    struct MockControl {
        records: RefCell<Vec<PublishedSearchBundleManifest>>,
        marks: RefCell<Vec<(PublishedGenerationSet, u64)>>,
    }

    impl PublishedSearchGenerationCatalogPort for MockControl {
        fn record_generation_manifest(
            &mut self,
            manifest: PublishedSearchBundleManifest,
        ) -> Result<(), CoreError> {
            self.records.borrow_mut().push(manifest);
            Ok(())
        }
    }

    impl PublishedSearchActivationStatePort for MockControl {
        fn mark_active_generation(
            &mut self,
            generation: &PublishedGenerationSet,
            active_at_ms: u64,
        ) -> Result<(), CoreError> {
            self.marks
                .borrow_mut()
                .push((generation.clone(), active_at_ms));
            Ok(())
        }
    }

    struct MockLexical {
        call_count: AtomicUsize,
        last_bytes_len: AtomicUsize,
        fail: bool,
    }

    impl SearchPlaneLexicalIndexBuildPort for MockLexical {
        fn build_lexical_index(
            &self,
            _manifest: &PublishedSearchBundleManifest,
            input: LexicalBuildInput<'_>,
        ) -> Result<(), CoreError> {
            let _previous_count = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.last_bytes_len
                .store(input.chunk_rows.len(), Ordering::SeqCst);
            if self.fail {
                Err(CoreError::Storage("mock lexical build failure".into()))
            } else {
                Ok(())
            }
        }
    }

    struct MockSemantic {
        call_count: AtomicUsize,
        had_embeddings: AtomicUsize,
        fail: bool,
    }

    impl SearchPlaneSemanticIndexBuildPort for MockSemantic {
        fn build_semantic_index(
            &self,
            _manifest: &PublishedSearchBundleManifest,
            input: SemanticBuildInput<'_>,
        ) -> Result<(), CoreError> {
            let _previous_count = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.had_embeddings.store(
                usize::from(input.embedding_records.is_some()),
                Ordering::SeqCst,
            );
            if self.fail {
                Err(CoreError::Storage("mock semantic build failure".into()))
            } else {
                Ok(())
            }
        }
    }

    fn write_artifact(root: &std::path::Path, relative: &str, body: &[u8]) -> BundleArtifactRef {
        let full = root.join(relative);
        if let Some(parent) = full.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            assert!(false, "create parent {parent:?}: {error}");
        }
        if let Err(error) = std::fs::write(&full, body) {
            assert!(false, "write artifact {full:?}: {error}");
        }
        let mut hasher = Sha256::new();
        hasher.update(body);
        let digest = hex_lowercase(&hasher.finalize());
        let byte_length = match u64::try_from(body.len()) {
            Ok(value) => value,
            Err(error) => {
                assert!(false, "body length overflow: {error}");
                0
            }
        };
        BundleArtifactRef {
            relative_path: relative.into(),
            encoding: BundleEncoding::Json,
            byte_length,
            content_digest: ManifestDigest::new(digest),
        }
    }

    fn sample_generation() -> PublishedGenerationSet {
        PublishedGenerationSet {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            lexical_generation: GenerationId::new(10),
            symbol_generation: GenerationId::new(11),
            structural_generation: None,
            history_generation: None,
            semantic_generation: Some(GenerationId::new(12)),
            metadata_generation: None,
        }
    }

    fn sample_manifest(
        chunk_ref: BundleArtifactRef,
        symbol_ref: BundleArtifactRef,
        embedding_ref: Option<BundleArtifactRef>,
    ) -> PublishedSearchBundleManifest {
        PublishedSearchBundleManifest {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            bundle_schema_version: 1,
            lexical_chunk_rows: chunk_ref,
            symbol_rows: symbol_ref,
            metadata_rows: None,
            graph_rows: None,
            embedding_input_views: None,
            embedding_records: embedding_ref,
            mutation_delta: None,
        }
    }

    #[test]
    fn happy_path_runs_builds_and_marks_active() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 123,
        };
        let outcome = ok_or_fail!(uc.materialize(manifest, generation.clone()), "materialize");
        assert_eq!(outcome, MaterializeOutcome::Activated);
        assert_eq!(lexical.call_count.load(Ordering::SeqCst), 1);
        assert_eq!(semantic.call_count.load(Ordering::SeqCst), 1);
        assert_eq!(semantic.had_embeddings.load(Ordering::SeqCst), 0);
        assert_eq!(control.records.borrow().len(), 1);
        let marks = control.marks.borrow();
        assert_eq!(marks.len(), 1);
        let Some(mark) = marks.first() else {
            assert!(false, "mark recorded but list empty");
            return;
        };
        assert_eq!(mark.0, generation);
        assert_eq!(mark.1, 123);
    }

    #[test]
    fn lexical_build_failure_blocks_activation() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: true,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 123,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::Storage(_))),
            "expected Storage error, got {result:?}"
        );
        assert!(control.records.borrow().is_empty());
        assert!(control.marks.borrow().is_empty());
    }

    #[test]
    fn semantic_build_failure_blocks_activation() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: true,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 123,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::Storage(_))),
            "expected Storage error, got {result:?}"
        );
        assert!(control.records.borrow().is_empty());
        assert!(control.marks.borrow().is_empty());
    }

    #[test]
    fn embedding_records_when_present_are_loaded() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let embedding = write_artifact(
            &bundle_root,
            "bundle/embedding.bin",
            b"vector payload bytes",
        );
        let manifest = sample_manifest(chunk, symbols, Some(embedding));
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 5,
        };
        let _outcome: MaterializeOutcome =
            ok_or_fail!(uc.materialize(manifest, generation), "materialize");
        assert_eq!(semantic.had_embeddings.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_mismatched_repo_id_between_manifest_and_generation() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let mut manifest = sample_manifest(chunk, symbols, None);
        manifest.repo_id = RepoId::new("other");
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 1,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
    }

    #[test]
    fn digest_mismatch_is_rejected_before_builds() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let mut chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        chunk.content_digest =
            ManifestDigest::new("0000000000000000000000000000000000000000000000000000000000000000");
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 1,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
        assert_eq!(lexical.call_count.load(Ordering::SeqCst), 0);
        assert_eq!(semantic.call_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn byte_length_mismatch_is_rejected() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let mut chunk = write_artifact(&bundle_root, "bundle/chunk.json", b"[]");
        chunk.byte_length = 999;
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 1,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
    }

    #[test]
    fn missing_artifact_file_is_storage_error() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = BundleArtifactRef {
            relative_path: "missing/chunk.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 2,
            content_digest: ManifestDigest::new("dead"),
        };
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 1,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::Storage(_))),
            "expected Storage, got {result:?}"
        );
    }

    #[test]
    fn path_traversal_in_relative_path_is_rejected_before_fs_access() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let evil = BundleArtifactRef {
            relative_path: "../etc/passwd".into(),
            encoding: BundleEncoding::Json,
            byte_length: 1,
            content_digest: ManifestDigest::new("d"),
        };
        let result = load_and_verify_artifact(&bundle_root, "manifest.lexical_chunk_rows", &evil);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
    }

    #[test]
    fn absolute_path_in_relative_path_is_rejected() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let evil = BundleArtifactRef {
            relative_path: "/etc/passwd".into(),
            encoding: BundleEncoding::Json,
            byte_length: 1,
            content_digest: ManifestDigest::new("d"),
        };
        let result = load_and_verify_artifact(&bundle_root, "manifest.lexical_chunk_rows", &evil);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
    }

    #[test]
    fn invalid_artifact_ref_shape_is_rejected_pre_io() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let bundle_root = dir.path().to_path_buf();
        let chunk = BundleArtifactRef {
            relative_path: "bundle/chunk.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 0, // invalid per BundlePolicy::validate_artifact_ref
            content_digest: ManifestDigest::new("d"),
        };
        let symbols = write_artifact(&bundle_root, "bundle/symbol.json", b"[]");
        let manifest = sample_manifest(chunk, symbols, None);
        let generation = sample_generation();

        let mut control = MockControl::default();
        let lexical = MockLexical {
            call_count: AtomicUsize::new(0),
            last_bytes_len: AtomicUsize::new(0),
            fail: false,
        };
        let semantic = MockSemantic {
            call_count: AtomicUsize::new(0),
            had_embeddings: AtomicUsize::new(0),
            fail: false,
        };
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &lexical,
            semantic: &semantic,
            bundle_root,
            now_ms: 1,
        };
        let result = uc.materialize(manifest, generation);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
        assert_eq!(lexical.call_count.load(Ordering::SeqCst), 0);
    }
}
