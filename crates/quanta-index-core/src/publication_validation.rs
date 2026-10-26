//! Non-persisted proof custody for one source-corpus publication.

use std::any::Any;
use std::sync::Arc;

use quanta_index_contract::{
    ManifestGeneration, SearchCorpusIngestBatch, SearchPlaneTrackKind, SourcePublicationBinding,
};

use crate::publication_admission::{
    PublicationAdmissionRequest, PublicationBuildWork, PublicationOutputAdmission,
    PublicationOutputPermit, PublicationOutputReservation, PublicationResourceAdmissionPort,
    PublicationResourcePermit,
};
use crate::{CoreError, IngestBatchFootprint, RequestBudgetV1};

/// One caller-owned publication scope, never a process or query cache.
///
/// This owner grants no validation by itself. An adapter may retain its private
/// validated proof type here only after proving the committed content. Every
/// subsequent use must authenticate the current bytes and inventory again.
/// Bodies, query handles and successful-response flags are not proof custody.
/// The owner is deliberately not Clone, serializable, or part of an IPC DTO.
#[derive(Default)]
pub struct PublicationValidationOwner {
    binding: Option<PublicationBodyBinding>,
    prepare_request_budget: Option<RequestBudgetV1>,
    source_allocation: Option<crate::SourceEventAllocationV1>,
    lexical_reservation: Option<crate::LexicalDocumentVersionReservationV1>,
    lexical_proof: Option<Box<dyn Any + Send + Sync>>,
    semantic_proof: Option<Box<dyn Any + Send + Sync>>,
    // Fields drop in declaration order. Keep the resource permits last so
    // adapter proofs and body custody are released before provisional capacity.
    publication_resources: Option<PublicationResourceState>,
    publication_control: Option<Arc<dyn PublicationResourcePermit>>,
}

struct PublicationResourceState {
    work: PublicationBuildWork,
    footprint: IngestBatchFootprint,
    permit: Arc<dyn PublicationResourcePermit>,
}

#[derive(Eq, PartialEq)]
struct PublicationBodyBinding {
    publication: SourcePublicationBinding,
    base_generation: Option<ManifestGeneration>,
    seal: bool,
    payload_sha256: [u8; 32],
}

impl PublicationValidationOwner {
    /// Retain the control allowance before the outer journal's first write.
    ///
    /// The dispatcher invokes this after terminal inspection and before
    /// prepare. Identical body reuse keeps the original allowance; a first
    /// control acquisition after work admission or source reservation refuses.
    pub fn admit_publication_control(
        &mut self,
        batch: &SearchCorpusIngestBatch,
        budget: &RequestBudgetV1,
        admission: &dyn PublicationResourceAdmissionPort,
    ) -> Result<(), CoreError> {
        self.bind_batch(batch)?;
        if self.publication_control.is_some() {
            return Ok(());
        }
        if self.publication_resources.is_some()
            || self.source_allocation.is_some()
            || self.lexical_reservation.is_some()
        {
            return Err(CoreError::InvalidContract(
                "publication control admission followed work or durable reservation".into(),
            ));
        }
        self.publication_control = Some(Arc::from(admission.try_acquire_control(batch, budget)?));
        Ok(())
    }

    /// Whether the outer journal/control allowance remains held.
    #[must_use]
    pub fn has_publication_control_permit(&self) -> bool {
        self.publication_control.is_some()
    }

    /// Acquire one non-clone resource permit for this exact publication.
    ///
    /// The first acquisition must precede source-event and document-version
    /// reservation. Repeating the same body, work, and footprint reuses the
    /// original permit without calling the port again.
    pub fn admit_publication_resources(
        &mut self,
        request: &PublicationAdmissionRequest<'_>,
        budget: &RequestBudgetV1,
        admission: &dyn PublicationResourceAdmissionPort,
    ) -> Result<(), CoreError> {
        self.bind_batch(request.batch)?;
        if let Some(held) = &self.publication_resources {
            if held.work != request.work || held.footprint != request.footprint {
                return Err(CoreError::InvalidContract(
                    "publication resource request changed within one owner".into(),
                ));
            }
            return Ok(());
        }
        if self.source_allocation.is_some() || self.lexical_reservation.is_some() {
            return Err(CoreError::InvalidContract(
                "publication resource admission followed durable reservation".into(),
            ));
        }
        let permit = admission.try_acquire(request, budget)?;
        self.publication_resources = Some(PublicationResourceState {
            work: request.work,
            footprint: request.footprint,
            permit: Arc::from(permit),
        });
        Ok(())
    }

    /// Whether this owner currently retains a publication resource permit.
    #[must_use]
    pub fn has_publication_resource_permit(&self) -> bool {
        self.publication_resources.is_some()
    }

    /// Borrow this publication's output authority without peer cancellation.
    #[must_use]
    pub fn output_admission(&self) -> PublicationOutputAdmission<'_> {
        PublicationOutputAdmission::new(
            self.publication_control.as_ref(),
            self.publication_resources
                .as_ref()
                .map(|state| &state.permit),
        )
    }

    /// Borrow one output reservation from the correct held allowance.
    ///
    /// `None` is only the explicitly unqualified legacy composition. Once
    /// either publication permit is installed, a missing permit for this class
    /// refuses rather than silently writing outside the resource ledger.
    pub fn reserve_publication_output(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Option<Box<dyn PublicationOutputPermit>>, CoreError> {
        self.output_admission().reserve(request)
    }

    /// Bind the optional, read-only preparation to the caller's request.
    /// This does not authorize interruption of an admitted durable ingest.
    /// Rebinding to a sibling child budget would exchange its independent
    /// cancellation scope, so only clones of the exact scope are accepted.
    pub fn bind_prepare_request_budget(
        &mut self,
        request: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        match &self.prepare_request_budget {
            Some(previous)
                if !previous.same_request_as(request)
                    || previous.deadline() != request.deadline() =>
            {
                Err(CoreError::InvalidContract(
                    "publication preparation request budget changed within one owner".into(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.prepare_request_budget = Some(request.clone());
                Ok(())
            }
        }
    }

    /// The set-once request budget for optional preparation only.
    #[must_use]
    pub fn prepare_request_budget(&self) -> Option<&RequestBudgetV1> {
        self.prepare_request_budget.as_ref()
    }

    /// Bind catalog-owned document IDs to the exact validated mutation body.
    pub fn bind_lexical_reservation(
        &mut self,
        batch: &SearchCorpusIngestBatch,
        reservation: &crate::LexicalDocumentVersionReservationV1,
    ) -> Result<(), CoreError> {
        reservation.validate()?;
        self.bind_source_allocation(batch, &reservation.allocation)?;
        let file_count = u64::try_from(batch.replace_scopes.len()).map_err(|error| {
            CoreError::InvalidContract(format!("source file count exceeds u64: {error}"))
        })?;
        let native_text_count = batch.replace_scopes.iter().try_fold(0u64, |total, scope| {
            let count = u64::try_from(scope.chunks.len()).map_err(|error| {
                CoreError::InvalidContract(format!("native text count exceeds u64: {error}"))
            })?;
            total
                .checked_add(count)
                .ok_or_else(|| CoreError::InvalidContract("native text count exceeds u64".into()))
        })?;
        if reservation.file.count != file_count
            || reservation.native_text.count != native_text_count
        {
            return Err(CoreError::InvalidContract(
                "lexical reservation differs from publication document counts".into(),
            ));
        }
        match &self.lexical_reservation {
            Some(previous) if previous != reservation => Err(CoreError::InvalidContract(
                "lexical document IDs changed within one publication".into(),
            )),
            Some(_) => Ok(()),
            None => {
                self.lexical_reservation = Some(reservation.clone());
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn lexical_reservation(&self) -> Option<&crate::LexicalDocumentVersionReservationV1> {
        self.lexical_reservation.as_ref()
    }

    /// Retain the original catalog allocation before component mutation.
    ///
    /// The materializer obtains this record from the source-publication catalog;
    /// neither a generation path nor a component watermark supplies incarnation.
    pub fn bind_source_allocation(
        &mut self,
        batch: &SearchCorpusIngestBatch,
        allocation: &crate::SourceEventAllocationV1,
    ) -> Result<(), CoreError> {
        self.bind_batch(batch)?;
        allocation.validate()?;
        if allocation.request.repo_id != batch.repo_id
            || allocation.request.revision_id != batch.revision_id
            || allocation.request.event != batch.source_event
            || allocation.generation != batch.generation
        {
            return Err(CoreError::InvalidContract(
                "publication allocation differs from the original source-corpus target".into(),
            ));
        }
        match &self.source_allocation {
            Some(previous) if previous != allocation => Err(CoreError::InvalidContract(
                "publication allocation changed within one mutation scope".into(),
            )),
            Some(_) => Ok(()),
            None => {
                self.source_allocation = Some(allocation.clone());
                Ok(())
            }
        }
    }

    /// Original durable allocation, available after materializer reservation.
    #[must_use]
    pub fn source_allocation(&self) -> Option<&crate::SourceEventAllocationV1> {
        self.source_allocation.as_ref()
    }

    /// Bind this scope before any physical validation or mutation. Reusing a
    /// publication owner for another body is a contract error, even when the
    /// two bodies happen to name the same generation.
    pub fn bind_batch(&mut self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        // The declared transport token alone cannot bind direct adapter calls.
        // Hash the actual mutation payload using its canonical contract owner;
        // containing target/base/seal fields are bound separately here.
        let binding = PublicationBodyBinding {
            publication: SourcePublicationBinding::for_batch(batch),
            base_generation: batch.base_generation,
            seal: batch.seal,
            payload_sha256: quanta_index_contract::source_event_payload_sha256(batch).map_err(
                |error| CoreError::InvalidContract(format!("publication validation body: {error}")),
            )?,
        };
        match &self.binding {
            Some(previous) if previous != &binding => Err(CoreError::InvalidContract(
                "publication validation owner belongs to another source-corpus batch".into(),
            )),
            Some(_) => Ok(()),
            None => {
                self.binding = Some(binding);
                Ok(())
            }
        }
    }

    /// Adapter-local, opaque proof custody. The adapter's concrete type and
    /// validated constructors remain outside core. Initializing its empty
    /// state conveys no authority; byte authentication is still mandatory.
    pub fn proof_state<T: Any + Default + Send + Sync>(
        &mut self,
        track: SearchPlaneTrackKind,
    ) -> Result<&mut T, CoreError> {
        let slot = match track {
            SearchPlaneTrackKind::Lexical => &mut self.lexical_proof,
            SearchPlaneTrackKind::Semantic => &mut self.semantic_proof,
            SearchPlaneTrackKind::Structural => {
                return Err(CoreError::InvalidContract(
                    "source-corpus publication proof custody has no structural track".into(),
                ));
            }
        };
        let proof = slot.get_or_insert_with(|| Box::<T>::default());
        proof.downcast_mut::<T>().ok_or_else(|| {
            CoreError::InvalidContract(
                "publication validation owner was used by a different proof producer".into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PublicationValidationOwner;
    use crate::publication_admission::{
        PublicationAdmissionRequest, PublicationBuildWork, PublicationOutputClass,
        PublicationOutputPermit, PublicationOutputReservation, PublicationResourceAdmissionPort,
        PublicationResourcePermit,
    };
    use crate::{
        CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestBatchFootprint, RequestBudgetV1,
        SourceEventAllocationRequestV1, SourceEventAllocationV1,
    };
    use quanta_index_contract::{
        BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch,
        SearchPlaneTrackKind, SourcePublicationEvent,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct TestPermit {
        drops: Arc<AtomicUsize>,
        log: Option<Arc<Mutex<Vec<&'static str>>>>,
        label: &'static str,
    }

    impl PublicationResourcePermit for TestPermit {
        fn held_file_allowance(
            &self,
        ) -> Result<Option<Arc<crate::publication_capacity::PublicationAllowance>>, CoreError>
        {
            Ok(None) // Explicitly unqualified scripted fixture.
        }

        fn reserve_output(
            &self,
            _request: PublicationOutputReservation,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            Err(CoreError::InvalidContract(
                "test admission has no output ledger".into(),
            ))
        }
    }

    impl Drop for TestPermit {
        fn drop(&mut self) {
            let _previous = self.drops.fetch_add(1, Ordering::SeqCst);
            if let Some(log) = &self.log {
                log.lock().expect("drop log").push(self.label);
            }
        }
    }

    struct TestAdmission {
        calls: AtomicUsize,
        control_calls: AtomicUsize,
        drops: Arc<AtomicUsize>,
        control_drops: Arc<AtomicUsize>,
        refuse: bool,
        log: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    impl TestAdmission {
        fn new(refuse: bool) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                control_calls: AtomicUsize::new(0),
                drops: Arc::new(AtomicUsize::new(0)),
                control_drops: Arc::new(AtomicUsize::new(0)),
                refuse,
                log: None,
            }
        }
    }

    impl PublicationResourceAdmissionPort for TestAdmission {
        fn try_acquire_control(
            &self,
            _batch: &SearchCorpusIngestBatch,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            let _previous = self.control_calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "scripted control refusal".into(),
                });
            }
            Ok(Box::new(TestPermit {
                drops: Arc::clone(&self.control_drops),
                log: self.log.clone(),
                label: "control",
            }))
        }

        fn try_acquire(
            &self,
            _request: &PublicationAdmissionRequest<'_>,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                return Err(CoreError::Typed {
                    code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "scripted resource refusal".into(),
                });
            }
            Ok(Box::new(TestPermit {
                drops: Arc::clone(&self.drops),
                log: self.log.clone(),
                label: "permit",
            }))
        }
    }

    // This fixture proves routing and lifetime only; it is not a capacity
    // ledger and is never installed by a production composition.
    struct RoutingAdmission {
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    struct RoutingPermit {
        label: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    struct RoutingOutputPermit {
        label: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl PublicationResourcePermit for RoutingPermit {
        fn held_file_allowance(
            &self,
        ) -> Result<Option<Arc<crate::publication_capacity::PublicationAllowance>>, CoreError>
        {
            Ok(None) // Explicitly unqualified routing fixture.
        }

        fn reserve_output(
            &self,
            _request: PublicationOutputReservation,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            self.log
                .lock()
                .expect("routing log")
                .push(match self.label {
                    "control" => "reserve control",
                    _ => "reserve work",
                });
            Ok(Box::new(RoutingOutputPermit {
                label: self.label,
                log: Arc::clone(&self.log),
            }))
        }
    }

    impl Drop for RoutingPermit {
        fn drop(&mut self) {
            self.log.lock().expect("routing log").push(self.label);
        }
    }

    impl PublicationOutputPermit for RoutingOutputPermit {
        fn cancel_rejected_created(&mut self, writer: std::fs::File) -> Result<(), CoreError> {
            self.retire_writer_preserving_pending(writer)?;
            Err(CoreError::InvalidContract(
                "unqualified fixture cannot prove rejected birth cancellation".into(),
            ))
        }
        fn retire_writer_preserving_pending(
            &mut self,
            writer: std::fs::File,
        ) -> Result<(), CoreError> {
            drop(writer); // Explicitly unqualified routing fixture.
            self.log
                .lock()
                .expect("routing log")
                .push("writer retired pending");
            Ok(())
        }

        fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
            if staging != Path::new("/owned/.staging") || target != Path::new("/owned/target") {
                return Err(CoreError::InvalidContract(
                    "scripted output paths did not reach child permit".into(),
                ));
            }
            self.log.lock().expect("routing log").push("child bound");
            Ok(())
        }

        fn creation_started(&mut self) -> Result<(), CoreError> {
            self.log.lock().expect("routing log").push("child started");
            Ok(())
        }

        fn created(&mut self, _writer: &std::fs::File, _staging: &Path) -> Result<(), CoreError> {
            Ok(())
        }

        fn before_replace(&mut self) -> Result<(), CoreError> {
            Ok(())
        }

        fn committed(
            &mut self,
            _writer: std::fs::File,
            _actual_encoded_bytes: u64,
        ) -> Result<(), CoreError> {
            self.log
                .lock()
                .expect("routing log")
                .push("child committed");
            Ok(())
        }

        fn rename_started(&mut self, target: &Path) -> Result<(), CoreError> {
            if target != Path::new("/final/target") {
                return Err(CoreError::InvalidContract(
                    "scripted rename target did not reach child permit".into(),
                ));
            }
            self.log.lock().expect("routing log").push("rename started");
            Ok(())
        }

        fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
            self.log.lock().expect("routing log").push("renamed synced");
            Ok(())
        }

        fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
            self.log.lock().expect("routing log").push("child absent");
            Ok(())
        }

        fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
            self.log.lock().expect("routing log").push("child absent");
            Ok(())
        }

        fn confirmed_safe_absent_after_writer_close(
            &mut self,
            _writer: std::fs::File,
        ) -> Result<(), CoreError> {
            self.log.lock().expect("routing log").push("child absent");
            Ok(())
        }
    }

    impl Drop for RoutingOutputPermit {
        fn drop(&mut self) {
            self.log
                .lock()
                .expect("routing log")
                .push(match self.label {
                    "control" => "child control",
                    _ => "child work",
                });
        }
    }

    impl PublicationResourceAdmissionPort for RoutingAdmission {
        fn try_acquire_control(
            &self,
            _batch: &SearchCorpusIngestBatch,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Ok(Box::new(RoutingPermit {
                label: "control",
                log: Arc::clone(&self.log),
            }))
        }

        fn try_acquire(
            &self,
            _request: &PublicationAdmissionRequest<'_>,
            _budget: &RequestBudgetV1,
        ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
            Ok(Box::new(RoutingPermit {
                label: "work",
                log: Arc::clone(&self.log),
            }))
        }
    }

    #[derive(Default)]
    struct ProofDrop {
        log: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    impl Drop for ProofDrop {
        fn drop(&mut self) {
            if let Some(log) = &self.log {
                log.lock().expect("drop log").push("proof");
            }
        }
    }

    fn request(batch: &SearchCorpusIngestBatch) -> PublicationAdmissionRequest<'_> {
        PublicationAdmissionRequest {
            batch,
            footprint: IngestBatchFootprint {
                carried_records: 0,
                embedded_records: 0,
                source_bytes: 0,
                text_bytes: 0,
                vector_bytes: 0,
            },
            work: PublicationBuildWork::Materialize {
                lexical: true,
                semantic: true,
            },
        }
    }

    fn batch() -> SearchCorpusIngestBatch {
        SearchCorpusIngestBatch {
            source_event: SourcePublicationEvent {
                stream_id: "proof-stream".into(),
                event_id: "proof-event".into(),
                expected_base_event_id: None,
                payload_sha256: [0; 32],
            },
            repo_id: RepoId::new("repo").expect("fixture repo"),
            revision_id: RevisionId::new("revision").expect("fixture revision"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "proof-manifest".into(),
            batch_digest: "0".repeat(64),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    #[test]
    fn output_reservation_validates_one_file_bounds_and_routes_with_parent_custody() {
        for (bytes, files) in [(0, 1), (1, 0), (8, 2), (8, u64::MAX)] {
            assert!(
                PublicationOutputReservation::new(PublicationOutputClass::Source, bytes, files)
                    .is_err()
            );
        }
        let control = PublicationOutputReservation::new(PublicationOutputClass::Control, 4, 1)
            .expect("control bounds");
        let source = PublicationOutputReservation::new(PublicationOutputClass::Source, 8, 1)
            .expect("source bounds");
        assert_eq!(control.max_encoded_bytes(), 4);
        assert_eq!(source.max_files(), 1);
        let budget = RequestBudgetV1::unbounded();
        let batch = batch();
        let log = Arc::new(Mutex::new(Vec::new()));
        let admission = RoutingAdmission {
            log: Arc::clone(&log),
        };
        let mut owner = PublicationValidationOwner::default();
        let unqualified = owner.output_admission();
        assert!(
            unqualified
                .reserve(source)
                .expect("unqualified composition")
                .is_none()
        );
        owner
            .admit_publication_control(&batch, &budget, &admission)
            .expect("control admission");
        assert!(matches!(
            owner.output_admission().reserve(source),
            Err(CoreError::InvalidContract(message)) if message.contains("no matching")
        ));
        let mut control_child = owner
            .output_admission()
            .reserve(control)
            .expect("control routing")
            .expect("qualified control child");
        owner
            .admit_publication_resources(&request(&batch), &budget, &admission)
            .expect("work admission");
        budget.cancel_handle().cancel();
        assert!(budget.is_cancelled());
        let mut work_child = owner
            .output_admission()
            .reserve(source)
            .expect("accepted work routing is independent of peer cancellation")
            .expect("qualified work child");
        assert_eq!(
            *log.lock().expect("routing log"),
            ["reserve control", "reserve work"]
        );
        drop(owner);
        assert_eq!(
            *log.lock().expect("routing log"),
            ["reserve control", "reserve work"]
        );
        control_child
            .bind_output(Path::new("/owned/.staging"), Path::new("/owned/target"))
            .expect("child forwards exact paths");
        control_child
            .creation_started()
            .expect("child forwards start");
        let witness = tempfile::tempfile().expect("routing witness");
        control_child
            .created(&witness, Path::new("/owned/.staging"))
            .expect("child forwards exact writer FD");
        control_child
            .committed(witness, 4)
            .expect("child forwards commit");
        control_child
            .rename_started(Path::new("/final/target"))
            .expect("child forwards prospective rename target");
        control_child
            .renamed_and_synced()
            .expect("child forwards inode transfer proof");
        work_child
            .bind_output(Path::new("/owned/.staging"), Path::new("/owned/target"))
            .expect("work child forwards exact paths");
        work_child
            .creation_started()
            .expect("work child forwards start");
        work_child
            .confirmed_uncreated_absent_and_synced()
            .expect("child forwards deletion proof");
        drop(control_child);
        drop(work_child);
        assert_eq!(
            *log.lock().expect("routing log"),
            [
                "reserve control",
                "reserve work",
                "child bound",
                "child started",
                "child committed",
                "rename started",
                "renamed synced",
                "child bound",
                "child started",
                "child absent",
                "child control",
                "control",
                "child work",
                "work",
            ]
        );
    }

    #[test]
    fn owned_work_authority_keeps_parent_alive_and_refuses_control() {
        let budget = RequestBudgetV1::unbounded();
        let batch = batch();
        let log = Arc::new(Mutex::new(Vec::new()));
        let admission = RoutingAdmission {
            log: Arc::clone(&log),
        };
        let mut owner = PublicationValidationOwner::default();
        assert!(
            owner
                .output_admission()
                .owned_work_admission()
                .expect("unqualified")
                .is_none()
        );
        owner
            .admit_publication_control(&batch, &budget, &admission)
            .expect("control");
        assert!(matches!(
            owner.output_admission().owned_work_admission(),
            Err(CoreError::InvalidContract(_))
        ));
        assert!(matches!(
            owner.output_admission().held_work_file_allowance(),
            Err(CoreError::InvalidContract(_))
        ));
        owner
            .admit_publication_resources(&request(&batch), &budget, &admission)
            .expect("work");
        let work = owner
            .output_admission()
            .owned_work_admission()
            .expect("work custody")
            .expect("work authority");
        let clone = work.clone();
        let control = PublicationOutputReservation::new(PublicationOutputClass::Control, 4, 1)
            .expect("control bound");
        assert!(matches!(
            work.reserve(control),
            Err(CoreError::InvalidContract(_))
        ));
        drop(owner);
        assert_eq!(*log.lock().expect("routing log"), ["control"]);
        let child = clone
            .reserve(
                PublicationOutputReservation::new(PublicationOutputClass::Lexical, 8, 1)
                    .expect("lexical bound"),
            )
            .expect("child");
        drop(work);
        drop(clone);
        assert_eq!(
            *log.lock().expect("routing log"),
            ["control", "reserve work"]
        );
        drop(child);
        assert_eq!(
            *log.lock().expect("routing log"),
            ["control", "reserve work", "child work", "work"]
        );
    }

    #[test]
    fn work_only_publication_refuses_control_output_without_legacy_fallback() {
        let budget = RequestBudgetV1::unbounded();
        let batch = batch();
        let log = Arc::new(Mutex::new(Vec::new()));
        let admission = RoutingAdmission {
            log: Arc::clone(&log),
        };
        let mut owner = PublicationValidationOwner::default();
        owner
            .admit_publication_resources(&request(&batch), &budget, &admission)
            .expect("work-only direct materializer");
        let control = PublicationOutputReservation::new(PublicationOutputClass::Control, 1, 1)
            .expect("control bounds");
        assert!(matches!(
            owner.reserve_publication_output(control),
            Err(CoreError::InvalidContract(message)) if message.contains("no matching")
        ));
        assert!(log.lock().expect("routing log").is_empty());
    }

    #[test]
    fn publication_control_is_set_once_and_outlives_refused_work_and_proofs() {
        let batch = batch();
        let budget = RequestBudgetV1::unbounded();
        let refusing = TestAdmission::new(true);
        let mut refused = PublicationValidationOwner::default();
        assert!(matches!(
            refused.admit_publication_control(&batch, &budget, &refusing),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ));
        assert!(!refused.has_publication_control_permit());
        assert!(!refused.has_publication_resource_permit());
        assert_eq!(refusing.control_drops.load(Ordering::SeqCst), 0);
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut port = TestAdmission::new(false);
        port.log = Some(Arc::clone(&log));
        let mut owner = PublicationValidationOwner::default();
        owner
            .admit_publication_control(&batch, &budget, &port)
            .expect("control before journal");
        owner
            .admit_publication_control(&batch, &budget, &port)
            .expect("same body reuses control");
        assert_eq!(port.control_calls.load(Ordering::SeqCst), 1);
        let mut changed = batch.clone();
        changed.bundle_payload = Some(b"different body".to_vec());
        assert!(matches!(
            owner.admit_publication_control(&changed, &budget, &port),
            Err(CoreError::InvalidContract(_))
        ));
        assert_eq!(port.control_calls.load(Ordering::SeqCst), 1);
        port.refuse = true;
        assert!(matches!(
            owner.admit_publication_resources(&request(&batch), &budget, &port),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ));
        assert!(owner.has_publication_control_permit());
        assert!(!owner.has_publication_resource_permit());
        assert_eq!(port.control_drops.load(Ordering::SeqCst), 0);
        owner
            .proof_state::<ProofDrop>(SearchPlaneTrackKind::Lexical)
            .expect("proof custody")
            .log = Some(Arc::clone(&log));
        drop(owner);
        assert_eq!(port.control_drops.load(Ordering::SeqCst), 1);
        assert_eq!(*log.lock().expect("drop order"), ["proof", "control"]);
    }

    #[test]
    fn publication_control_refuses_late_acquisition_and_both_permits_drop_after_proof() {
        let batch = batch();
        let budget = RequestBudgetV1::unbounded();
        let mut port = TestAdmission::new(false);
        let mut late = PublicationValidationOwner::default();
        late.admit_publication_resources(&request(&batch), &budget, &port)
            .expect("component-level work admission");
        assert!(matches!(
            late.admit_publication_control(&batch, &budget, &port),
            Err(CoreError::InvalidContract(message)) if message.contains("followed work")
        ));
        assert_eq!(port.control_calls.load(Ordering::SeqCst), 0);
        drop(late);

        let log = Arc::new(Mutex::new(Vec::new()));
        port.log = Some(Arc::clone(&log));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut owner = PublicationValidationOwner::default();
            owner
                .admit_publication_control(&batch, &budget, &port)
                .expect("control admission");
            owner
                .admit_publication_resources(&request(&batch), &budget, &port)
                .expect("work admission");
            owner
                .proof_state::<ProofDrop>(SearchPlaneTrackKind::Lexical)
                .expect("proof custody")
                .log = Some(Arc::clone(&log));
            std::panic::resume_unwind(Box::new("scripted publication unwind"));
        }));
        assert!(outcome.is_err());
        assert_eq!(
            *log.lock().expect("drop order"),
            ["proof", "permit", "control"]
        );
    }

    #[test]
    fn resource_permit_is_set_once_for_exact_body_work_and_footprint() {
        let batch = batch();
        let request = request(&batch);
        let port = TestAdmission::new(false);
        let budget = RequestBudgetV1::unbounded();
        let mut owner = PublicationValidationOwner::default();
        assert!(!owner.has_publication_resource_permit());
        owner
            .admit_publication_resources(&request, &budget, &port)
            .expect("initial acquisition");
        owner
            .admit_publication_resources(&request, &budget, &port)
            .expect("same request reuses permit");
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
        assert!(owner.has_publication_resource_permit());

        let changed_work = PublicationAdmissionRequest {
            work: PublicationBuildWork::FinalizeOnly,
            ..request
        };
        assert!(matches!(
            owner.admit_publication_resources(&changed_work, &budget, &port),
            Err(CoreError::InvalidContract(message))
                if message.contains("resource request changed")
        ));
        let changed_footprint = PublicationAdmissionRequest {
            footprint: IngestBatchFootprint {
                carried_records: 1,
                ..request.footprint
            },
            ..request
        };
        assert!(matches!(
            owner.admit_publication_resources(&changed_footprint, &budget, &port),
            Err(CoreError::InvalidContract(message))
                if message.contains("resource request changed")
        ));
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
        drop(owner);
        assert_eq!(port.drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn resource_acquisition_refuses_late_or_changed_body_before_port_call() {
        let batch = batch();
        let port = TestAdmission::new(false);
        let budget = RequestBudgetV1::unbounded();
        let allocation = SourceEventAllocationV1 {
            request: SourceEventAllocationRequestV1 {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                event: batch.source_event.clone(),
            },
            generation: batch.generation,
            root_incarnation: [1; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
        };
        let mut late = PublicationValidationOwner::default();
        late.bind_source_allocation(&batch, &allocation)
            .expect("durable allocation binding");
        assert!(matches!(
            late.admit_publication_resources(&request(&batch), &budget, &port),
            Err(CoreError::InvalidContract(message))
                if message.contains("followed durable reservation")
        ));
        assert_eq!(port.calls.load(Ordering::SeqCst), 0);
        assert!(!late.has_publication_resource_permit());

        let mut owner = PublicationValidationOwner::default();
        owner.bind_batch(&batch).expect("first body");
        let mut changed = batch.clone();
        changed.bundle_payload = Some(b"different body".to_vec());
        assert!(matches!(
            owner.admit_publication_resources(&request(&changed), &budget, &port),
            Err(CoreError::InvalidContract(message))
                if message.contains("another source-corpus batch")
        ));
        assert_eq!(port.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn refused_acquisition_leaves_no_permit_and_unwind_drops_proof_before_permit() {
        let batch = batch();
        let budget = RequestBudgetV1::unbounded();
        let refusing = TestAdmission::new(true);
        let mut refused = PublicationValidationOwner::default();
        assert!(matches!(
            refused.admit_publication_resources(&request(&batch), &budget, &refusing),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ));
        assert!(!refused.has_publication_resource_permit());
        assert_eq!(refusing.drops.load(Ordering::SeqCst), 0);

        let log = Arc::new(Mutex::new(Vec::new()));
        let mut admitted = TestAdmission::new(false);
        admitted.log = Some(Arc::clone(&log));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut owner = PublicationValidationOwner::default();
            owner
                .admit_publication_resources(&request(&batch), &budget, &admitted)
                .expect("admit before unwind");
            owner
                .proof_state::<ProofDrop>(SearchPlaneTrackKind::Lexical)
                .expect("proof custody")
                .log = Some(Arc::clone(&log));
            std::panic::resume_unwind(Box::new("scripted publication unwind"));
        }));
        assert!(outcome.is_err());
        assert_eq!(admitted.drops.load(Ordering::SeqCst), 1);
        assert_eq!(*log.lock().expect("drop order"), ["proof", "permit"]);
    }

    #[test]
    fn preparation_budget_rebind_requires_the_same_cancel_scope_and_deadline() {
        let mut owner = PublicationValidationOwner::default();
        let deadline = Instant::now() + Duration::from_secs(60);
        let parent = RequestBudgetV1::until(deadline);
        let first = parent.child();
        let sibling = parent.child();
        owner
            .bind_prepare_request_budget(&first)
            .expect("first binding");
        owner
            .bind_prepare_request_budget(&first.clone())
            .expect("same scope binding");
        assert!(matches!(
            owner.bind_prepare_request_budget(&sibling),
            Err(crate::CoreError::InvalidContract(message))
                if message.contains("preparation request budget changed")
        ));
        assert!(matches!(
            owner.bind_prepare_request_budget(&RequestBudgetV1::until(deadline)),
            Err(crate::CoreError::InvalidContract(message))
                if message.contains("preparation request budget changed")
        ));
        let retained = owner.prepare_request_budget().expect("original retained");
        assert!(retained.same_request_as(&first));
        assert_eq!(retained.deadline(), deadline);
        first.cancel_handle().cancel();
        assert!(retained.is_cancelled());
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn actual_body_changes_refuse_even_with_unchanged_declared_tokens() {
        let original = batch();
        for change in 0..4 {
            let mut owner = PublicationValidationOwner::default();
            owner.bind_batch(&original).expect("initial binding");
            owner.bind_batch(&original).expect("same body binding");
            let mut changed = original.clone();
            match change {
                0 => changed.bundle_payload = Some(b"different mutation bytes".to_vec()),
                1 => changed.base_generation = Some(ManifestGeneration::new(7)),
                2 => changed.seal = false,
                _ => changed.manifest_digest = "different-manifest".into(),
            }
            assert!(
                matches!(owner.bind_batch(&changed), Err(crate::CoreError::InvalidContract(message))
                if message.contains("belongs to another source-corpus batch"))
            );
        }
    }

    #[test]
    fn producer_substitution_refuses_without_erasing_a_track_proof() {
        let mut owner = PublicationValidationOwner::default();
        assert!(
            matches!(owner.proof_state::<u64>(SearchPlaneTrackKind::Structural),
            Err(crate::CoreError::InvalidContract(message)) if message.contains("no structural track"))
        );
        *owner
            .proof_state::<u64>(SearchPlaneTrackKind::Lexical)
            .expect("lexical state") = 7;
        assert!(
            matches!(owner.proof_state::<String>(SearchPlaneTrackKind::Lexical),
            Err(crate::CoreError::InvalidContract(message)) if message.contains("different proof producer"))
        );
        assert_eq!(
            *owner
                .proof_state::<u64>(SearchPlaneTrackKind::Lexical)
                .expect("retained state"),
            7
        );
        *owner
            .proof_state::<String>(SearchPlaneTrackKind::Semantic)
            .expect("separate semantic state") = "semantic".into();
        assert_eq!(
            *owner
                .proof_state::<u64>(SearchPlaneTrackKind::Lexical)
                .expect("independent lexical state"),
            7
        );
        assert_eq!(
            owner
                .proof_state::<String>(SearchPlaneTrackKind::Semantic)
                .expect("retained semantic state")
                .as_str(),
            "semantic"
        );
    }
}
