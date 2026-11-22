#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextStatistics {
    /// Number of selected owners in the field, including documents with zero tokens.
    pub docs: u64,
    /// Indexable NFC runs counted once, regardless of the two case surfaces.
    pub indexed_tokens: u64,
}
