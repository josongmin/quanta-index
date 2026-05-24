//! Channel-specific identifiers. Newtypes are intentional: they prevent mixing
//! op-key domains (chunk vs symbol vs embedding) across tracks.
//!
//! The newtype shapes here are produced by the crate-local `string_newtype!`
//! and `u64_newtype!` macros (see `src/macros.rs`). The macros expand to
//! manual `serde::Serialize` / `serde::Deserialize` impls — proc-macro
//! `#[derive]` for serde is banned workspace-wide.

u64_newtype!(ChannelSeq);

impl ChannelSeq {
    /// Saturating successor — never wraps. Used to step a write cursor without
    /// admitting the arithmetic-overflow lint into the call sites.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

string_newtype!(ChunkId);
string_newtype!(SymbolId);
string_newtype!(EmbeddingId);
