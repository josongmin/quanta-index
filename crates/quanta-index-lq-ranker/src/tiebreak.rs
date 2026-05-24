//! Stable tiebreak ordering for ranked candidates.
//!
//! [`rank_candidates`] sorts a vector of [`ScoredCandidate`] into the
//! canonical ranker output order, using a six-component key derived from
//! [`TiebreakKey`]:
//!
//! 1. `score` — descending via [`f32::total_cmp`] (so `NaN`-like patterns are
//!    deterministic, though `score` is already clamped to `[0.0, 1.0]` by
//!    [`crate::scorer::CompositeScorer::score`]);
//! 2. `repo_id` ascending — repository identity tiebreak;
//! 3. `generation` ascending — older generations win for byte-identical
//!    scores so that activation cutovers do not perturb output order;
//! 4. `repo_relative_path` ascending — lexicographic on the path bytes;
//! 5. `start_line` ascending — earliest hit wins;
//! 6. `doc_id` ascending — last-resort deterministic tiebreak.
//!
//! The sort is stable; the key is total over [`ScoredCandidate`], so the
//! output sequence is deterministic regardless of input order.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::cmp::Ordering;
use core::fmt;

use crate::signals::CandidateSignals;

/// One candidate carrying its composed score plus its tiebreak identity.
///
/// Field ordering matches the canonical tiebreak tier order so that
/// downstream readers can audit `Debug` output against the documented
/// composition.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoredCandidate {
    /// Globally unique document identifier.
    pub doc_id: u64,
    /// Repository identifier (tiebreak tier 2).
    pub repo_id: u64,
    /// Repository generation (tiebreak tier 3).
    pub generation: u64,
    /// Repository-relative path (tiebreak tier 4).
    pub repo_relative_path: Box<str>,
    /// Starting line of the hit (tiebreak tier 5).
    pub start_line: u32,
    /// Composed score from [`crate::scorer::CompositeScorer::score`].
    pub score: f32,
    /// Per-signal envelope that produced `score`.
    pub signals: CandidateSignals,
}

impl ScoredCandidate {
    /// Borrow the tiebreak key for this candidate.
    #[must_use]
    pub fn tiebreak_key(&self) -> TiebreakKey<'_> {
        TiebreakKey {
            score: self.score,
            repo_id: self.repo_id,
            generation: self.generation,
            repo_relative_path: &self.repo_relative_path,
            start_line: self.start_line,
            doc_id: self.doc_id,
        }
    }
}

/// Borrowed tiebreak key over a [`ScoredCandidate`].
///
/// `Ord` is total: `score` is compared via [`f32::total_cmp`] descending,
/// then the remaining tiers ascend.
#[derive(Clone, Copy, Debug)]
pub struct TiebreakKey<'a> {
    /// Composed score (descending).
    pub score: f32,
    /// Repository identifier (ascending).
    pub repo_id: u64,
    /// Generation (ascending).
    pub generation: u64,
    /// Path bytes (ascending lex).
    pub repo_relative_path: &'a str,
    /// Starting line (ascending).
    pub start_line: u32,
    /// Document id (ascending).
    pub doc_id: u64,
}

impl PartialEq for TiebreakKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for TiebreakKey<'_> {}

impl PartialOrd for TiebreakKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TiebreakKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Score descends — invert the natural ordering of `total_cmp`.
        match other.score.total_cmp(&self.score) {
            Ordering::Equal => {}
            non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
        }
        match self.repo_id.cmp(&other.repo_id) {
            Ordering::Equal => {}
            non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
        }
        match self.generation.cmp(&other.generation) {
            Ordering::Equal => {}
            non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
        }
        match self.repo_relative_path.cmp(other.repo_relative_path) {
            Ordering::Equal => {}
            non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
        }
        match self.start_line.cmp(&other.start_line) {
            Ordering::Equal => {}
            non_eq @ (Ordering::Less | Ordering::Greater) => return non_eq,
        }
        self.doc_id.cmp(&other.doc_id)
    }
}

impl fmt::Display for TiebreakKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TiebreakKey(score={}, repo_id={}, generation={}, path={}, start_line={}, doc_id={})",
            self.score,
            self.repo_id,
            self.generation,
            self.repo_relative_path,
            self.start_line,
            self.doc_id,
        )
    }
}

/// Sort `v` into ranker output order via [`TiebreakKey`].
///
/// The sort is stable; the key is total over [`ScoredCandidate`].
#[must_use]
pub fn rank_candidates(mut v: Vec<ScoredCandidate>) -> Vec<ScoredCandidate> {
    v.sort_by(|a, b| a.tiebreak_key().cmp(&b.tiebreak_key()));
    v
}

impl serde::Serialize for ScoredCandidate {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(7))?;
        m.serialize_entry("doc_id", &self.doc_id)?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("repo_id", &self.repo_id)?;
        m.serialize_entry("repo_relative_path", self.repo_relative_path.as_ref())?;
        m.serialize_entry("score", &self.score)?;
        m.serialize_entry("signals", &self.signals)?;
        m.serialize_entry("start_line", &self.start_line)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for ScoredCandidate {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = ScoredCandidate;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ScoredCandidate map with seven fields")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<ScoredCandidate, M::Error> {
                let mut doc_id: Option<u64> = None;
                let mut repo_id: Option<u64> = None;
                let mut generation: Option<u64> = None;
                let mut repo_relative_path: Option<String> = None;
                let mut start_line: Option<u32> = None;
                let mut score: Option<f32> = None;
                let mut signals: Option<CandidateSignals> = None;
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
                        "score" => {
                            if score.is_some() {
                                return Err(serde::de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        "signals" => {
                            if signals.is_some() {
                                return Err(serde::de::Error::duplicate_field("signals"));
                            }
                            signals = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &[
                                    "doc_id",
                                    "generation",
                                    "repo_id",
                                    "repo_relative_path",
                                    "score",
                                    "signals",
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
                let score = score.ok_or_else(|| serde::de::Error::missing_field("score"))?;
                let signals = signals.ok_or_else(|| serde::de::Error::missing_field("signals"))?;
                Ok(ScoredCandidate {
                    doc_id,
                    repo_id,
                    generation,
                    repo_relative_path: repo_relative_path.into_boxed_str(),
                    start_line,
                    score,
                    signals,
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{ScoredCandidate, rank_candidates};
    use crate::signals::CandidateSignals;

    fn cand(
        doc_id: u64,
        repo_id: u64,
        generation: u64,
        path: &str,
        start_line: u32,
        score: f32,
    ) -> ScoredCandidate {
        ScoredCandidate {
            doc_id,
            repo_id,
            generation,
            repo_relative_path: Box::<str>::from(path),
            start_line,
            score,
            signals: CandidateSignals::identity(),
        }
    }

    fn ids(v: &[ScoredCandidate]) -> Vec<u64> {
        v.iter().map(|c| c.doc_id).collect()
    }

    #[test]
    fn score_desc_dominates_all_other_tiers() {
        let v = vec![
            cand(1, 9, 9, "z", 9, 0.10),
            cand(2, 1, 1, "a", 1, 0.90),
            cand(3, 5, 5, "m", 5, 0.50),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![2, 3, 1]);
    }

    #[test]
    fn repo_id_breaks_tie_when_score_equal() {
        let v = vec![
            cand(1, 9, 1, "a", 1, 0.50),
            cand(2, 3, 1, "a", 1, 0.50),
            cand(3, 7, 1, "a", 1, 0.50),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![2, 3, 1]);
    }

    #[test]
    fn generation_breaks_tie_when_score_and_repo_equal() {
        let v = vec![
            cand(1, 4, 5, "a", 1, 0.50),
            cand(2, 4, 2, "a", 1, 0.50),
            cand(3, 4, 7, "a", 1, 0.50),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![2, 1, 3]);
    }

    #[test]
    fn path_breaks_tie_when_score_repo_generation_equal() {
        let v = vec![
            cand(1, 4, 2, "src/zeta.rs", 1, 0.50),
            cand(2, 4, 2, "src/alpha.rs", 1, 0.50),
            cand(3, 4, 2, "src/mid.rs", 1, 0.50),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![2, 3, 1]);
    }

    #[test]
    fn start_line_breaks_tie_when_path_also_equal() {
        let v = vec![
            cand(1, 4, 2, "src/a.rs", 99, 0.50),
            cand(2, 4, 2, "src/a.rs", 2, 0.50),
            cand(3, 4, 2, "src/a.rs", 33, 0.50),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![2, 3, 1]);
    }

    #[test]
    fn doc_id_breaks_final_tie() {
        let v = vec![
            cand(7, 4, 2, "src/a.rs", 1, 0.50),
            cand(2, 4, 2, "src/a.rs", 1, 0.50),
            cand(5, 4, 2, "src/a.rs", 1, 0.50),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![2, 5, 7]);
    }

    #[test]
    fn five_candidate_mixed_tier_ordering() {
        // Designed so every tier breaks at least one pair:
        //   * doc 10: score 0.9 (unique top)
        //   * doc 20: score 0.5, repo 3 (beats 30/40/50)
        //   * doc 30: score 0.5, repo 5, generation 1 (beats 40)
        //   * doc 40: score 0.5, repo 5, generation 2, path "a/b"
        //   * doc 50: score 0.5, repo 5, generation 2, path "a/c"
        let v = vec![
            cand(50, 5, 2, "a/c", 1, 0.5),
            cand(40, 5, 2, "a/b", 1, 0.5),
            cand(30, 5, 1, "a/c", 1, 0.5),
            cand(20, 3, 9, "z", 9, 0.5),
            cand(10, 9, 9, "z", 9, 0.9),
        ];
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![10, 20, 30, 40, 50]);
    }

    #[test]
    fn stable_sort_preserves_input_order_on_full_tie() {
        let v = vec![
            cand(1, 1, 1, "a", 1, 0.5),
            cand(2, 1, 1, "a", 1, 0.5),
            cand(3, 1, 1, "a", 1, 0.5),
        ];
        // Inputs are already in doc_id ascending; final-tier doc_id ascending
        // confirms determinism either way.
        let got = rank_candidates(v);
        assert_eq!(ids(&got), vec![1, 2, 3]);
    }

    #[test]
    fn serde_roundtrip_via_ciborium() {
        let c = cand(42, 7, 3, "src/lib.rs", 11, 0.625);
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&c, &mut buf) {
            Ok(()) => {}
            Err(e) => assert!(false, "serialize: {e}"),
        }
        match ciborium::de::from_reader::<ScoredCandidate, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, c),
            Err(e) => assert!(false, "deserialize: {e}"),
        }
    }
}
