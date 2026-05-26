use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{LexicalCandidate, ManifestGeneration, RepoId, RevisionId, StructuralCandidate};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeTarget {
    CodeQl,
}

impl BridgeTarget {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodeQl => "codeql",
        }
    }
}

impl Serialize for BridgeTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct BridgeTargetVisitor;

impl Visitor<'_> for BridgeTargetVisitor {
    type Value = BridgeTarget;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BridgeTarget string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "codeql" => Ok(BridgeTarget::CodeQl),
            other => Err(de::Error::unknown_variant(other, &["codeql"])),
        }
    }
}

impl<'de> Deserialize<'de> for BridgeTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(BridgeTargetVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeScope {
    Lexical,
    Structural,
    Semantic,
    Hybrid,
}

impl BridgeScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Structural => "structural",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

impl Serialize for BridgeScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct BridgeScopeVisitor;

impl Visitor<'_> for BridgeScopeVisitor {
    type Value = BridgeScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BridgeScope string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "lexical" => Ok(BridgeScope::Lexical),
            "structural" => Ok(BridgeScope::Structural),
            "semantic" => Ok(BridgeScope::Semantic),
            "hybrid" => Ok(BridgeScope::Hybrid),
            other => Err(de::Error::unknown_variant(
                other,
                &["lexical", "structural", "semantic", "hybrid"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for BridgeScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(BridgeScopeVisitor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BridgeCandidate {
    Lexical(LexicalCandidate),
    Structural(StructuralCandidate),
}

const BRIDGE_CANDIDATE_FIELDS: &[&str] = &["kind", "lexical", "structural"];

impl Serialize for BridgeCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BridgeCandidate", 2)?;
        match self {
            Self::Lexical(candidate) => {
                state.serialize_field("kind", "lexical")?;
                state.serialize_field("lexical", candidate)?;
            }
            Self::Structural(candidate) => {
                state.serialize_field("kind", "structural")?;
                state.serialize_field("structural", candidate)?;
            }
        }
        state.end()
    }
}

struct BridgeCandidateVisitor;

impl<'de> Visitor<'de> for BridgeCandidateVisitor {
    type Value = BridgeCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BridgeCandidate map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut lexical: Option<LexicalCandidate> = None;
        let mut structural: Option<StructuralCandidate> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => kind = Some(map.next_value()?),
                "lexical" => lexical = Some(map.next_value()?),
                "structural" => structural = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, BRIDGE_CANDIDATE_FIELDS)),
            }
        }
        match kind.as_deref() {
            Some("lexical") => Ok(BridgeCandidate::Lexical(
                lexical.ok_or_else(|| de::Error::missing_field("lexical"))?,
            )),
            Some("structural") => Ok(BridgeCandidate::Structural(
                structural.ok_or_else(|| de::Error::missing_field("structural"))?,
            )),
            Some(other) => Err(de::Error::unknown_variant(
                other,
                &["lexical", "structural"],
            )),
            None => Err(de::Error::missing_field("kind")),
        }
    }
}

impl<'de> Deserialize<'de> for BridgeCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BridgeCandidate",
            BRIDGE_CANDIDATE_FIELDS,
            BridgeCandidateVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BridgeCandidatePacket {
    pub target: BridgeTarget,
    pub scope: BridgeScope,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub source_syntax: Option<String>,
    pub translator_version: Option<String>,
    pub candidates: Vec<BridgeCandidate>,
}

const BRIDGE_CANDIDATE_PACKET_FIELDS: &[&str] = &[
    "target",
    "scope",
    "repo_id",
    "revision_id",
    "manifest_generation",
    "source_syntax",
    "translator_version",
    "candidates",
];

impl Serialize for BridgeCandidatePacket {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 6;
        if self.source_syntax.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.translator_version.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("BridgeCandidatePacket", field_count)?;
        state.serialize_field("target", &self.target)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        if let Some(source_syntax) = &self.source_syntax {
            state.serialize_field("source_syntax", source_syntax)?;
        }
        if let Some(translator_version) = &self.translator_version {
            state.serialize_field("translator_version", translator_version)?;
        }
        state.serialize_field("candidates", &self.candidates)?;
        state.end()
    }
}

struct BridgeCandidatePacketVisitor;

impl<'de> Visitor<'de> for BridgeCandidatePacketVisitor {
    type Value = BridgeCandidatePacket;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BridgeCandidatePacket map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut target: Option<BridgeTarget> = None;
        let mut scope: Option<BridgeScope> = None;
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut source_syntax: Option<Option<String>> = None;
        let mut translator_version: Option<Option<String>> = None;
        let mut candidates: Option<Vec<BridgeCandidate>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "target" => target = Some(map.next_value()?),
                "scope" => scope = Some(map.next_value()?),
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "manifest_generation" => manifest_generation = Some(map.next_value()?),
                "source_syntax" => source_syntax = Some(Some(map.next_value()?)),
                "translator_version" => translator_version = Some(Some(map.next_value()?)),
                "candidates" => candidates = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        BRIDGE_CANDIDATE_PACKET_FIELDS,
                    ));
                }
            }
        }
        Ok(BridgeCandidatePacket {
            target: target.ok_or_else(|| de::Error::missing_field("target"))?,
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            source_syntax: source_syntax.unwrap_or(None),
            translator_version: translator_version.unwrap_or(None),
            candidates: candidates.ok_or_else(|| de::Error::missing_field("candidates"))?,
        })
    }
}

impl<'de> Deserialize<'de> for BridgeCandidatePacket {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BridgeCandidatePacket",
            BRIDGE_CANDIDATE_PACKET_FIELDS,
            BridgeCandidatePacketVisitor,
        )
    }
}
