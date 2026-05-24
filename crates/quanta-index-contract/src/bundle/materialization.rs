use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{FileId, ManifestGeneration, RepoId, RepoRelativePath, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportEdge {
    pub from_symbol: String,
    pub to_symbol: String,
}

const IMPORT_EDGE_FIELDS: &[&str] = &["from_symbol", "to_symbol"];

impl Serialize for ImportEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ImportEdge", 2)?;
        state.serialize_field("from_symbol", &self.from_symbol)?;
        state.serialize_field("to_symbol", &self.to_symbol)?;
        state.end()
    }
}

struct ImportEdgeVisitor;

impl<'de> Visitor<'de> for ImportEdgeVisitor {
    type Value = ImportEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ImportEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut from_symbol: Option<String> = None;
        let mut to_symbol: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "from_symbol" => {
                    if from_symbol.is_some() {
                        return Err(de::Error::duplicate_field("from_symbol"));
                    }
                    from_symbol = Some(map.next_value()?);
                }
                "to_symbol" => {
                    if to_symbol.is_some() {
                        return Err(de::Error::duplicate_field("to_symbol"));
                    }
                    to_symbol = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, IMPORT_EDGE_FIELDS)),
            }
        }
        let from_symbol = from_symbol.ok_or_else(|| de::Error::missing_field("from_symbol"))?;
        let to_symbol = to_symbol.ok_or_else(|| de::Error::missing_field("to_symbol"))?;
        Ok(ImportEdge {
            from_symbol,
            to_symbol,
        })
    }
}

impl<'de> Deserialize<'de> for ImportEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("ImportEdge", IMPORT_EDGE_FIELDS, ImportEdgeVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallEdge {
    pub caller_symbol: String,
    pub callee_symbol: String,
}

const CALL_EDGE_FIELDS: &[&str] = &["caller_symbol", "callee_symbol"];

impl Serialize for CallEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CallEdge", 2)?;
        state.serialize_field("caller_symbol", &self.caller_symbol)?;
        state.serialize_field("callee_symbol", &self.callee_symbol)?;
        state.end()
    }
}

struct CallEdgeVisitor;

impl<'de> Visitor<'de> for CallEdgeVisitor {
    type Value = CallEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CallEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_acc: Option<String> = None;
        let mut target_acc: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "caller_symbol" => {
                    if source_acc.is_some() {
                        return Err(de::Error::duplicate_field("caller_symbol"));
                    }
                    source_acc = Some(map.next_value()?);
                }
                "callee_symbol" => {
                    if target_acc.is_some() {
                        return Err(de::Error::duplicate_field("callee_symbol"));
                    }
                    target_acc = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, CALL_EDGE_FIELDS)),
            }
        }
        Ok(CallEdge {
            caller_symbol: source_acc.ok_or_else(|| de::Error::missing_field("caller_symbol"))?,
            callee_symbol: target_acc.ok_or_else(|| de::Error::missing_field("callee_symbol"))?,
        })
    }
}

impl<'de> Deserialize<'de> for CallEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("CallEdge", CALL_EDGE_FIELDS, CallEdgeVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseOutput {
    pub language: String,
    pub parser_revision: String,
}

const PARSE_OUTPUT_FIELDS: &[&str] = &["language", "parser_revision"];

impl Serialize for ParseOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ParseOutput", 2)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("parser_revision", &self.parser_revision)?;
        state.end()
    }
}

struct ParseOutputVisitor;

impl<'de> Visitor<'de> for ParseOutputVisitor {
    type Value = ParseOutput;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ParseOutput map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut language: Option<String> = None;
        let mut parser_revision: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "language" => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(map.next_value()?);
                }
                "parser_revision" => {
                    if parser_revision.is_some() {
                        return Err(de::Error::duplicate_field("parser_revision"));
                    }
                    parser_revision = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, PARSE_OUTPUT_FIELDS)),
            }
        }
        let language = language.ok_or_else(|| de::Error::missing_field("language"))?;
        let parser_revision =
            parser_revision.ok_or_else(|| de::Error::missing_field("parser_revision"))?;
        Ok(ParseOutput {
            language,
            parser_revision,
        })
    }
}

impl<'de> Deserialize<'de> for ParseOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("ParseOutput", PARSE_OUTPUT_FIELDS, ParseOutputVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirOutput {
    pub root_kind: String,
    pub digest: String,
}

const HIR_OUTPUT_FIELDS: &[&str] = &["root_kind", "digest"];

impl Serialize for HirOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HirOutput", 2)?;
        state.serialize_field("root_kind", &self.root_kind)?;
        state.serialize_field("digest", &self.digest)?;
        state.end()
    }
}

struct HirOutputVisitor;

impl<'de> Visitor<'de> for HirOutputVisitor {
    type Value = HirOutput;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HirOutput map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut root_kind: Option<String> = None;
        let mut digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "root_kind" => {
                    if root_kind.is_some() {
                        return Err(de::Error::duplicate_field("root_kind"));
                    }
                    root_kind = Some(map.next_value()?);
                }
                "digest" => {
                    if digest.is_some() {
                        return Err(de::Error::duplicate_field("digest"));
                    }
                    digest = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HIR_OUTPUT_FIELDS)),
            }
        }
        let root_kind = root_kind.ok_or_else(|| de::Error::missing_field("root_kind"))?;
        let digest = digest.ok_or_else(|| de::Error::missing_field("digest"))?;
        Ok(HirOutput { root_kind, digest })
    }
}

impl<'de> Deserialize<'de> for HirOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("HirOutput", HIR_OUTPUT_FIELDS, HirOutputVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemIndexOutput {
    pub item_count: u32,
    pub digest: String,
}

const ITEM_INDEX_OUTPUT_FIELDS: &[&str] = &["item_count", "digest"];

impl Serialize for ItemIndexOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ItemIndexOutput", 2)?;
        state.serialize_field("item_count", &self.item_count)?;
        state.serialize_field("digest", &self.digest)?;
        state.end()
    }
}

struct ItemIndexOutputVisitor;

impl<'de> Visitor<'de> for ItemIndexOutputVisitor {
    type Value = ItemIndexOutput;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ItemIndexOutput map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut item_count: Option<u32> = None;
        let mut digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "item_count" => {
                    if item_count.is_some() {
                        return Err(de::Error::duplicate_field("item_count"));
                    }
                    item_count = Some(map.next_value()?);
                }
                "digest" => {
                    if digest.is_some() {
                        return Err(de::Error::duplicate_field("digest"));
                    }
                    digest = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, ITEM_INDEX_OUTPUT_FIELDS)),
            }
        }
        let item_count = item_count.ok_or_else(|| de::Error::missing_field("item_count"))?;
        let digest = digest.ok_or_else(|| de::Error::missing_field("digest"))?;
        Ok(ItemIndexOutput { item_count, digest })
    }
}

impl<'de> Deserialize<'de> for ItemIndexOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ItemIndexOutput",
            ITEM_INDEX_OUTPUT_FIELDS,
            ItemIndexOutputVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildContextOutput {
    pub profile_name: String,
    pub digest: String,
}

const BUILD_CONTEXT_OUTPUT_FIELDS: &[&str] = &["profile_name", "digest"];

impl Serialize for BuildContextOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BuildContextOutput", 2)?;
        state.serialize_field("profile_name", &self.profile_name)?;
        state.serialize_field("digest", &self.digest)?;
        state.end()
    }
}

struct BuildContextOutputVisitor;

impl<'de> Visitor<'de> for BuildContextOutputVisitor {
    type Value = BuildContextOutput;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BuildContextOutput map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut profile_name: Option<String> = None;
        let mut digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "profile_name" => {
                    if profile_name.is_some() {
                        return Err(de::Error::duplicate_field("profile_name"));
                    }
                    profile_name = Some(map.next_value()?);
                }
                "digest" => {
                    if digest.is_some() {
                        return Err(de::Error::duplicate_field("digest"));
                    }
                    digest = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, BUILD_CONTEXT_OUTPUT_FIELDS)),
            }
        }
        let profile_name = profile_name.ok_or_else(|| de::Error::missing_field("profile_name"))?;
        let digest = digest.ok_or_else(|| de::Error::missing_field("digest"))?;
        Ok(BuildContextOutput {
            profile_name,
            digest,
        })
    }
}

impl<'de> Deserialize<'de> for BuildContextOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BuildContextOutput",
            BUILD_CONTEXT_OUTPUT_FIELDS,
            BuildContextOutputVisitor,
        )
    }
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

const FILE_MATERIALIZATION_PACKET_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "file_id",
    "file_path",
    "file_text",
    "parse_output",
    "hir_output",
    "item_index",
    "build_context",
    "import_edges",
    "call_edges",
];

impl Serialize for FileMaterializationPacket {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 11;
        if self.build_context.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("FileMaterializationPacket", field_count)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("file_id", &self.file_id)?;
        state.serialize_field("file_path", &self.file_path)?;
        state.serialize_field("file_text", &self.file_text)?;
        state.serialize_field("parse_output", &self.parse_output)?;
        state.serialize_field("hir_output", &self.hir_output)?;
        state.serialize_field("item_index", &self.item_index)?;
        if let Some(build_context) = &self.build_context {
            state.serialize_field("build_context", build_context)?;
        }
        state.serialize_field("import_edges", &self.import_edges)?;
        state.serialize_field("call_edges", &self.call_edges)?;
        state.end()
    }
}

struct FileMaterializationPacketVisitor;

impl<'de> Visitor<'de> for FileMaterializationPacketVisitor {
    type Value = FileMaterializationPacket;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileMaterializationPacket map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut file_id: Option<FileId> = None;
        let mut file_path: Option<RepoRelativePath> = None;
        let mut file_text: Option<String> = None;
        let mut parse_output: Option<ParseOutput> = None;
        let mut hir_output: Option<HirOutput> = None;
        let mut item_index: Option<ItemIndexOutput> = None;
        let mut build_context: Option<BuildContextOutput> = None;
        let mut build_context_seen = false;
        let mut import_edges: Option<Vec<ImportEdge>> = None;
        let mut call_edges: Option<Vec<CallEdge>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                "file_id" => {
                    if file_id.is_some() {
                        return Err(de::Error::duplicate_field("file_id"));
                    }
                    file_id = Some(map.next_value()?);
                }
                "file_path" => {
                    if file_path.is_some() {
                        return Err(de::Error::duplicate_field("file_path"));
                    }
                    file_path = Some(map.next_value()?);
                }
                "file_text" => {
                    if file_text.is_some() {
                        return Err(de::Error::duplicate_field("file_text"));
                    }
                    file_text = Some(map.next_value()?);
                }
                "parse_output" => {
                    if parse_output.is_some() {
                        return Err(de::Error::duplicate_field("parse_output"));
                    }
                    parse_output = Some(map.next_value()?);
                }
                "hir_output" => {
                    if hir_output.is_some() {
                        return Err(de::Error::duplicate_field("hir_output"));
                    }
                    hir_output = Some(map.next_value()?);
                }
                "item_index" => {
                    if item_index.is_some() {
                        return Err(de::Error::duplicate_field("item_index"));
                    }
                    item_index = Some(map.next_value()?);
                }
                "build_context" => {
                    if build_context_seen {
                        return Err(de::Error::duplicate_field("build_context"));
                    }
                    build_context_seen = true;
                    build_context = Some(map.next_value()?);
                }
                "import_edges" => {
                    if import_edges.is_some() {
                        return Err(de::Error::duplicate_field("import_edges"));
                    }
                    import_edges = Some(map.next_value()?);
                }
                "call_edges" => {
                    if call_edges.is_some() {
                        return Err(de::Error::duplicate_field("call_edges"));
                    }
                    call_edges = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_MATERIALIZATION_PACKET_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let file_id = file_id.ok_or_else(|| de::Error::missing_field("file_id"))?;
        let file_path = file_path.ok_or_else(|| de::Error::missing_field("file_path"))?;
        let file_text = file_text.ok_or_else(|| de::Error::missing_field("file_text"))?;
        let parse_output = parse_output.ok_or_else(|| de::Error::missing_field("parse_output"))?;
        let hir_output = hir_output.ok_or_else(|| de::Error::missing_field("hir_output"))?;
        let item_index = item_index.ok_or_else(|| de::Error::missing_field("item_index"))?;
        let import_edges = import_edges.ok_or_else(|| de::Error::missing_field("import_edges"))?;
        let call_edges = call_edges.ok_or_else(|| de::Error::missing_field("call_edges"))?;
        Ok(FileMaterializationPacket {
            repo_id,
            revision_id,
            manifest_generation,
            file_id,
            file_path,
            file_text,
            parse_output,
            hir_output,
            item_index,
            build_context,
            import_edges,
            call_edges,
        })
    }
}

impl<'de> Deserialize<'de> for FileMaterializationPacket {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileMaterializationPacket",
            FILE_MATERIALIZATION_PACKET_FIELDS,
            FileMaterializationPacketVisitor,
        )
    }
}
