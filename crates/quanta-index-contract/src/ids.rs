//! Public ID newtypes shared between the producer and the search-plane.
//!
//! Each newtype below collapses to a one-line macro invocation. The
//! `string_newtype!` and `u64_newtype!` macros (see `src/macros.rs`) expand
//! to the same hand-written `serde::Serialize` / `serde::Deserialize` blocks
//! that previously lived here verbatim. Proc-macro derive is banned
//! workspace-wide; the declarative macros stay crate-local.

string_newtype!(RepoId);
string_newtype!(RevisionId);
u64_newtype!(ManifestGeneration);
u64_newtype!(GenerationId);
string_newtype!(ManifestDigest);
string_newtype!(FileId);
string_newtype!(RepoRelativePath);
