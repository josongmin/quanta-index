//! Identity newtypes + canonical candidate reference for hybrid fusion.
//!
//! Per SEM-02 §4.5, the extended merge-determinism tuple references
//! `(repo_id, manifest_generation, repo_relative_path, start_line, candidate_id)`
//! — the [`CandidateRef`] carries exactly those load-bearing identity
//! components, mirroring the ranker's `ScoredCandidate` identity tier.
//!
//! Newtypes (`DocId`, `RepoId`, `ManifestGeneration`) are deliberate to
//! prevent accidental cross-domain identity confusion (a `RepoId` cannot
//! flow into a `DocId` slot by type alone).
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Globally unique candidate document identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for DocId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DocId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = u64::deserialize(de)?;
        Ok(Self(v))
    }
}

/// Repository identifier (merge-tuple tier 4 per SEM-02 §4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId(pub u64);

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for RepoId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RepoId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = u64::deserialize(de)?;
        Ok(Self(v))
    }
}

/// Manifest generation pin (merge-tuple tier 5 per SEM-02 §4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ManifestGeneration(pub u64);

impl fmt::Display for ManifestGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for ManifestGeneration {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ManifestGeneration {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = u64::deserialize(de)?;
        Ok(Self(v))
    }
}

/// Canonical identity of a candidate carried through fusion.
///
/// Carries the five fields that the SEM-02 §4.5 merge-determinism tuple
/// uses as identity tie-breakers below `fused_score / lex_score / sem_score`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CandidateRef {
    /// Globally unique document id.
    pub doc_id: DocId,
    /// Repository id (merge-tuple tier 4).
    pub repo_id: RepoId,
    /// Manifest generation (merge-tuple tier 5).
    pub generation: ManifestGeneration,
    /// Repository-relative path (merge-tuple tier 6).
    pub repo_relative_path: Box<str>,
    /// Hit start line (merge-tuple tier 7).
    pub start_line: u32,
}

impl serde::Serialize for CandidateRef {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(5))?;
        m.serialize_entry("doc_id", &self.doc_id)?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("repo_id", &self.repo_id)?;
        m.serialize_entry("repo_relative_path", self.repo_relative_path.as_ref())?;
        m.serialize_entry("start_line", &self.start_line)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for CandidateRef {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = CandidateRef;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("CandidateRef map with five fields")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<CandidateRef, M::Error> {
                let mut doc_id: Option<DocId> = None;
                let mut repo_id: Option<RepoId> = None;
                let mut generation: Option<ManifestGeneration> = None;
                let mut repo_relative_path: Option<String> = None;
                let mut start_line: Option<u32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "doc_id" => {
                            if doc_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("doc_id"));
                            }
                            doc_id = Some(map.next_value()?);
                        }
                        "repo_id" => {
                            if repo_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("repo_id"));
                            }
                            repo_id = Some(map.next_value()?);
                        }
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "repo_relative_path" => {
                            if repo_relative_path.is_some() {
                                return Err(serde::de::Error::duplicate_field(
                                    "repo_relative_path",
                                ));
                            }
                            repo_relative_path = Some(map.next_value()?);
                        }
                        "start_line" => {
                            if start_line.is_some() {
                                return Err(serde::de::Error::duplicate_field("start_line"));
                            }
                            start_line = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &[
                                    "doc_id",
                                    "generation",
                                    "repo_id",
                                    "repo_relative_path",
                                    "start_line",
                                ],
                            ));
                        }
                    }
                }
                let doc_id = doc_id.ok_or_else(|| serde::de::Error::missing_field("doc_id"))?;
                let repo_id = repo_id.ok_or_else(|| serde::de::Error::missing_field("repo_id"))?;
                let generation =
                    generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                let repo_relative_path = repo_relative_path
                    .ok_or_else(|| serde::de::Error::missing_field("repo_relative_path"))?;
                let start_line =
                    start_line.ok_or_else(|| serde::de::Error::missing_field("start_line"))?;
                Ok(CandidateRef {
                    doc_id,
                    repo_id,
                    generation,
                    repo_relative_path: repo_relative_path.into_boxed_str(),
                    start_line,
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateRef, DocId, ManifestGeneration, RepoId};

    fn cref(doc: u64, repo: u64, gen_: u64, path: &str, line: u32) -> CandidateRef {
        CandidateRef {
            doc_id: DocId(doc),
            repo_id: RepoId(repo),
            generation: ManifestGeneration(gen_),
            repo_relative_path: Box::<str>::from(path),
            start_line: line,
        }
    }

    #[test]
    fn newtype_serde_roundtrip_doc_id() {
        let v = DocId(42);
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&v, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "serialize: {e}"),
        }
        match ciborium::de::from_reader::<DocId, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, v),
            Err(e) => assert!(false, "deserialize: {e}"),
        }
    }

    #[test]
    fn newtype_serde_roundtrip_repo_id() {
        let v = RepoId(7);
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&v, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "serialize: {e}"),
        }
        match ciborium::de::from_reader::<RepoId, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, v),
            Err(e) => assert!(false, "deserialize: {e}"),
        }
    }

    #[test]
    fn newtype_serde_roundtrip_generation() {
        let v = ManifestGeneration(3);
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&v, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "serialize: {e}"),
        }
        match ciborium::de::from_reader::<ManifestGeneration, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, v),
            Err(e) => assert!(false, "deserialize: {e}"),
        }
    }

    #[test]
    fn candidate_ref_serde_roundtrip() {
        let c = cref(11, 22, 33, "src/lib.rs", 44);
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&c, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "serialize: {e}"),
        }
        match ciborium::de::from_reader::<CandidateRef, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, c),
            Err(e) => assert!(false, "deserialize: {e}"),
        }
    }

    #[test]
    fn newtype_display() {
        assert_eq!(format!("{}", DocId(5)), "5");
        assert_eq!(format!("{}", RepoId(6)), "6");
        assert_eq!(format!("{}", ManifestGeneration(7)), "7");
    }
}
