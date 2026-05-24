//! Per-engine candidate carriers + the per-row hybrid contribution.
//!
//! Per SEM-02 §3.1, each engine delivers a list of `(CandidateRef, rank,
//! score)` triples to the fusion stage. After fusion, every fused row
//! carries an [`HybridContribution`] that records the rank/score from each
//! side (or `None` for an engine that did not return the row) plus the
//! `fused_score`. [`HybridContribution`] is the `SearchExplanation` v2
//! per-row payload from SEM-02 §4.1.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::types::CandidateRef;

/// One row from the lexical sub-query.
#[derive(Clone, Debug, PartialEq)]
pub struct LexCandidate {
    /// Identity tuple of this candidate (merge tiers 4..8).
    pub candidate_ref: CandidateRef,
    /// 1-indexed rank within the lexical engine's top-k.
    pub rank: u32,
    /// Lexical score (e.g. BM25); must be finite when consumed by fusion.
    pub score: f32,
}

impl serde::Serialize for LexCandidate {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        m.serialize_entry("candidate_ref", &self.candidate_ref)?;
        m.serialize_entry("rank", &self.rank)?;
        m.serialize_entry("score", &self.score)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LexCandidate {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = LexCandidate;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LexCandidate map with candidate_ref/rank/score")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<LexCandidate, M::Error> {
                let mut candidate_ref: Option<CandidateRef> = None;
                let mut rank: Option<u32> = None;
                let mut score: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "candidate_ref" => {
                            if candidate_ref.is_some() {
                                return Err(serde::de::Error::duplicate_field("candidate_ref"));
                            }
                            candidate_ref = Some(map.next_value()?);
                        }
                        "rank" => {
                            if rank.is_some() {
                                return Err(serde::de::Error::duplicate_field("rank"));
                            }
                            rank = Some(map.next_value()?);
                        }
                        "score" => {
                            if score.is_some() {
                                return Err(serde::de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["candidate_ref", "rank", "score"],
                            ));
                        }
                    }
                }
                let candidate_ref = candidate_ref
                    .ok_or_else(|| serde::de::Error::missing_field("candidate_ref"))?;
                let rank = rank.ok_or_else(|| serde::de::Error::missing_field("rank"))?;
                let score = score.ok_or_else(|| serde::de::Error::missing_field("score"))?;
                Ok(LexCandidate {
                    candidate_ref,
                    rank,
                    score,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// One row from the semantic sub-query.
#[derive(Clone, Debug, PartialEq)]
pub struct SemCandidate {
    /// Identity tuple of this candidate (merge tiers 4..8).
    pub candidate_ref: CandidateRef,
    /// 1-indexed rank within the semantic engine's top-k.
    pub rank: u32,
    /// Semantic score (e.g. cosine similarity); must be finite when consumed.
    pub score: f32,
}

impl serde::Serialize for SemCandidate {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        m.serialize_entry("candidate_ref", &self.candidate_ref)?;
        m.serialize_entry("rank", &self.rank)?;
        m.serialize_entry("score", &self.score)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for SemCandidate {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SemCandidate;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SemCandidate map with candidate_ref/rank/score")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SemCandidate, M::Error> {
                let mut candidate_ref: Option<CandidateRef> = None;
                let mut rank: Option<u32> = None;
                let mut score: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "candidate_ref" => {
                            if candidate_ref.is_some() {
                                return Err(serde::de::Error::duplicate_field("candidate_ref"));
                            }
                            candidate_ref = Some(map.next_value()?);
                        }
                        "rank" => {
                            if rank.is_some() {
                                return Err(serde::de::Error::duplicate_field("rank"));
                            }
                            rank = Some(map.next_value()?);
                        }
                        "score" => {
                            if score.is_some() {
                                return Err(serde::de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["candidate_ref", "rank", "score"],
                            ));
                        }
                    }
                }
                let candidate_ref = candidate_ref
                    .ok_or_else(|| serde::de::Error::missing_field("candidate_ref"))?;
                let rank = rank.ok_or_else(|| serde::de::Error::missing_field("rank"))?;
                let score = score.ok_or_else(|| serde::de::Error::missing_field("score"))?;
                Ok(SemCandidate {
                    candidate_ref,
                    rank,
                    score,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// Per-row hybrid contribution carried in `SearchExplanation` v2.
///
/// `lex_rank` / `lex_score` are `None` when the candidate appeared only on
/// the semantic side, and vice versa. `fused_score` is always populated
/// and is the value used for the §4.5 merge-tuple primary key.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridContribution {
    /// Identity tuple of this candidate (merge tiers 4..8).
    pub candidate_ref: CandidateRef,
    /// Lexical-side 1-indexed rank, or `None` when absent on lex side.
    pub lex_rank: Option<u32>,
    /// Lexical-side score, or `None` when absent on lex side.
    pub lex_score: Option<f32>,
    /// Semantic-side 1-indexed rank, or `None` when absent on sem side.
    pub sem_rank: Option<u32>,
    /// Semantic-side score, or `None` when absent on sem side.
    pub sem_score: Option<f32>,
    /// Fused score after applying the chosen [`crate::FusionStrategy`].
    pub fused_score: f32,
}

impl serde::Serialize for HybridContribution {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(6))?;
        m.serialize_entry("candidate_ref", &self.candidate_ref)?;
        m.serialize_entry("fused_score", &self.fused_score)?;
        m.serialize_entry("lex_rank", &self.lex_rank)?;
        m.serialize_entry("lex_score", &self.lex_score)?;
        m.serialize_entry("sem_rank", &self.sem_rank)?;
        m.serialize_entry("sem_score", &self.sem_score)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for HybridContribution {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HybridContribution;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HybridContribution map with six fields")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HybridContribution, M::Error> {
                let mut candidate_ref: Option<CandidateRef> = None;
                let mut lex_rank: Option<Option<u32>> = None;
                let mut lex_score: Option<Option<f32>> = None;
                let mut sem_rank: Option<Option<u32>> = None;
                let mut sem_score: Option<Option<f32>> = None;
                let mut fused_score: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "candidate_ref" => {
                            if candidate_ref.is_some() {
                                return Err(serde::de::Error::duplicate_field("candidate_ref"));
                            }
                            candidate_ref = Some(map.next_value()?);
                        }
                        "lex_rank" => {
                            if lex_rank.is_some() {
                                return Err(serde::de::Error::duplicate_field("lex_rank"));
                            }
                            lex_rank = Some(map.next_value()?);
                        }
                        "lex_score" => {
                            if lex_score.is_some() {
                                return Err(serde::de::Error::duplicate_field("lex_score"));
                            }
                            lex_score = Some(map.next_value()?);
                        }
                        "sem_rank" => {
                            if sem_rank.is_some() {
                                return Err(serde::de::Error::duplicate_field("sem_rank"));
                            }
                            sem_rank = Some(map.next_value()?);
                        }
                        "sem_score" => {
                            if sem_score.is_some() {
                                return Err(serde::de::Error::duplicate_field("sem_score"));
                            }
                            sem_score = Some(map.next_value()?);
                        }
                        "fused_score" => {
                            if fused_score.is_some() {
                                return Err(serde::de::Error::duplicate_field("fused_score"));
                            }
                            fused_score = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &[
                                    "candidate_ref",
                                    "fused_score",
                                    "lex_rank",
                                    "lex_score",
                                    "sem_rank",
                                    "sem_score",
                                ],
                            ));
                        }
                    }
                }
                let candidate_ref = candidate_ref
                    .ok_or_else(|| serde::de::Error::missing_field("candidate_ref"))?;
                let lex_rank =
                    lex_rank.ok_or_else(|| serde::de::Error::missing_field("lex_rank"))?;
                let lex_score =
                    lex_score.ok_or_else(|| serde::de::Error::missing_field("lex_score"))?;
                let sem_rank =
                    sem_rank.ok_or_else(|| serde::de::Error::missing_field("sem_rank"))?;
                let sem_score =
                    sem_score.ok_or_else(|| serde::de::Error::missing_field("sem_score"))?;
                let fused_score =
                    fused_score.ok_or_else(|| serde::de::Error::missing_field("fused_score"))?;
                Ok(HybridContribution {
                    candidate_ref,
                    lex_rank,
                    lex_score,
                    sem_rank,
                    sem_score,
                    fused_score,
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{HybridContribution, LexCandidate, SemCandidate};
    use crate::types::{CandidateRef, DocId, ManifestGeneration, RepoId};

    fn cref() -> CandidateRef {
        CandidateRef {
            doc_id: DocId(1),
            repo_id: RepoId(2),
            generation: ManifestGeneration(3),
            repo_relative_path: Box::<str>::from("src/lib.rs"),
            start_line: 10,
        }
    }

    #[test]
    fn lex_candidate_serde_roundtrip() {
        let c = LexCandidate {
            candidate_ref: cref(),
            rank: 1,
            score: 0.75,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&c, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<LexCandidate, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, c),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn sem_candidate_serde_roundtrip() {
        let c = SemCandidate {
            candidate_ref: cref(),
            rank: 3,
            score: 0.92,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&c, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<SemCandidate, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, c),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hybrid_contribution_both_sides_roundtrip() {
        let h = HybridContribution {
            candidate_ref: cref(),
            lex_rank: Some(2),
            lex_score: Some(0.60),
            sem_rank: Some(1),
            sem_score: Some(0.95),
            fused_score: 0.025,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&h, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<HybridContribution, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, h),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hybrid_contribution_lex_only_roundtrip() {
        let h = HybridContribution {
            candidate_ref: cref(),
            lex_rank: Some(2),
            lex_score: Some(0.60),
            sem_rank: None,
            sem_score: None,
            fused_score: 0.015,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&h, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<HybridContribution, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, h),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hybrid_contribution_sem_only_roundtrip() {
        let h = HybridContribution {
            candidate_ref: cref(),
            lex_rank: None,
            lex_score: None,
            sem_rank: Some(1),
            sem_score: Some(0.95),
            fused_score: 0.015,
        };
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&h, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match ciborium::de::from_reader::<HybridContribution, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, h),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
