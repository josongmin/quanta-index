//! Bounded transport staging. It owns no source-event or generation authority.

use quanta_index_contract::{
    SearchCorpusIngestBatch, SourcePublicationUploadAck, SourcePublicationUploadIdentity,
    SourcePublicationUploadPart,
};

use crate::{CoreError, RequestBudgetV1};

pub trait SourcePublicationUploadPort: Send + Sync {
    fn stage(&self, part: &SourcePublicationUploadPart, budget: &RequestBudgetV1)
        -> Result<SourcePublicationUploadAck, CoreError>;
    fn load(&self, identity: SourcePublicationUploadIdentity, budget: &RequestBudgetV1)
        -> Result<SearchCorpusIngestBatch, CoreError>;
    fn discard(&self, identity: SourcePublicationUploadIdentity) -> Result<(), CoreError>;
}
