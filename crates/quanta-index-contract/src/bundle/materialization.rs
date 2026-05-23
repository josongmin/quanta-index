use crate::{FileId, ManifestGeneration, RepoId, RepoRelativePath, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportEdge {
    pub from_symbol: String,
    pub to_symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallEdge {
    pub caller_symbol: String,
    pub callee_symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseOutput {
    pub language: String,
    pub parser_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirOutput {
    pub root_kind: String,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemIndexOutput {
    pub item_count: u32,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildContextOutput {
    pub profile_name: String,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMaterializationPacket {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub file_id: FileId,
    pub file_path: RepoRelativePath,
    pub file_text: String,
    pub parse_output: ParseOutput,
    pub hir_output: HirOutput,
    pub item_index: ItemIndexOutput,
    pub build_context: Option<BuildContextOutput>,
    pub import_edges: Vec<ImportEdge>,
    pub call_edges: Vec<CallEdge>,
}
