//! Bounded transport staging. It owns no source-event or generation authority.

use quanta_index_contract::{
    SearchCorpusIngestBatch, SourcePublicationUploadAck, SourcePublicationUploadIdentity,
    SourcePublicationUploadPart,
};

use crate::publication_admission::{PublicationControlFileMutation, PublicationControlOpenTicket};
use crate::publication_capacity::InodeKey;
use crate::{CoreError, RequestBudgetV1};
use std::path::Path;

/// Service-owned physical custody for transport bodies before an event exists.
///
/// The implementation shares the leased root's sealed publication ledger.
/// No publication event, generation, or authority is minted by this port.
pub trait SourcePublicationUploadFileAdmissionPort: Send + Sync {
    /// Authenticate the exact upload root before directory creation and settle
    /// retired physical mutations before rediscovering body pathnames.
    fn validate_root_and_reconcile(&self, upload_root: &Path) -> Result<(), CoreError>;

    /// Hold the declared complete body once, and subdivide only this write's
    /// exact growth. `None` requires first birth; `Some` requires the same
    /// retained file at the observed width. Existing boot inventory is already
    /// charged: restart may reserve only remaining bytes and zero file slots.
    /// The returned writer owns the original FD through checked retirement.
    fn begin_write(
        &self,
        path: &Path,
        declared_body_bytes: u64,
        expected_length: Option<u64>,
        target_end: u64,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError>;

    /// Reserve actual FD custody before a nofollow body read. The read-only
    /// ticket grants no creation, growth or event Control allowance.
    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError>;

    /// Delete only the inode/width observed on the original held reader FD.
    /// Failed unlink/sync retains physical custody; unused body allowance is
    /// released only after the owned namespace transition settles.
    fn delete_file_and_sync(
        &self,
        path: &Path,
        expected_inode: InodeKey,
        expected_length: u64,
    ) -> Result<(), CoreError>;
}

pub trait SourcePublicationUploadPort: Send + Sync {
    fn stage(
        &self,
        part: &SourcePublicationUploadPart,
        budget: &RequestBudgetV1,
    ) -> Result<SourcePublicationUploadAck, CoreError>;
    fn load(
        &self,
        identity: SourcePublicationUploadIdentity,
        budget: &RequestBudgetV1,
    ) -> Result<SearchCorpusIngestBatch, CoreError>;
    fn discard(&self, identity: SourcePublicationUploadIdentity) -> Result<(), CoreError>;
}
