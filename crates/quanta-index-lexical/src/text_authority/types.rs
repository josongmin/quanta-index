//! Native publication inputs and accounting shared with the adapter.

// The old directory name remains reserved by the outer generation verifier.
pub(crate) const TEXT_AUTHORITY_DIR_NAME: &str = "text-authority";

/// What one publish derived, retired, wrote, and carried.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TextAuthorityWriteReceipt {
    pub(crate) docs_derived: u64,
    pub(crate) docs_retired: u64,
    /// Inherited live documents decoded and retokenized by foreground compaction.
    pub(crate) compaction_rederived_docs: u64,
    /// Indexed-text bytes in those inherited documents.
    pub(crate) compaction_rederived_text_bytes: u64,
    pub(crate) shards_written: u64,
    pub(crate) shards_inherited: u64,
    pub(crate) posting_pages_written: u64,
    pub(crate) oversized_posting_pages: u64,
    pub(crate) oversized_posting_bytes: u64,
}

/// One text document assigned a durable logical ID by the source event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AddedTextDoc {
    pub(crate) doc_id: u64,
    pub(crate) source_file: quanta_index_contract::SourceFileKey,
    pub(crate) candidate_id: String,
    pub(crate) text: String,
}
