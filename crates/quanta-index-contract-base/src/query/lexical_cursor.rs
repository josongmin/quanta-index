//! The ranked lexical routes' row order and keyset cursor (QI-BB-005 보완 #4).
//!
//! Text and symbol rows are ranked. A page is ordered by score descending,
//! then repo-relative path, start line, end line and candidate id
//! ascending (strings byte-wise) — [`LexicalRowOrderKey::order`]. Within
//! one sealed generation that is a total order over the rows: the scores
//! are a pure function of the sealed index and the candidate id is unique.
//!
//! A [`LexicalCursor`] names the last row a page returned under that
//! order; the next page holds the rows strictly after it. It is a boundary,
//! not a lookup: a key that names no row still positions the page. Scores
//! are only comparable within the generation that produced them, so the
//! cursor names that generation, and a continuation against any other is
//! refused typed rather than served from a different ranking.

use core::cmp::Ordering;
use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::ids::{ManifestGeneration, RepoRelativePath};
use crate::results::{LexicalCandidate, QueryResultWindowV1};

/// Typed refusal for a continuation whose cursor was cut from another
/// generation than the one the request resolves to.
pub const QUERY_CURSOR_GENERATION_MISMATCH_CODE: &str = "QUERY_CURSOR_GENERATION_MISMATCH";

/// Typed refusal for a cursor on a text request whose route does not page.
///
/// A semantic scope, a hybrid lane, an explain, a structural or runtime
/// text leaf rank no pages: only text and symbol pages continue.
pub const QUERY_CURSOR_UNSUPPORTED_CODE: &str = "QUERY_CURSOR_UNSUPPORTED";

/// One ranked lexical row's position in the page order.
#[derive(Clone, Copy, Debug)]
pub struct LexicalRowOrderKey<'a> {
    pub score: f32,
    pub repo_relative_path: &'a str,
    pub start_line: u32,
    pub end_line: u32,
    pub candidate_id: &'a str,
}

impl LexicalRowOrderKey<'_> {
    /// `Less` when `self` comes first in a page: the higher score, then
    /// the lower path, start line, end line and candidate id.
    #[must_use]
    pub fn order(&self, other: &LexicalRowOrderKey<'_>) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.repo_relative_path.cmp(other.repo_relative_path))
            .then(self.start_line.cmp(&other.start_line))
            .then(self.end_line.cmp(&other.end_line))
            .then_with(|| self.candidate_id.cmp(other.candidate_id))
    }
}

impl LexicalCandidate {
    /// This row's position in the ranked lexical page order.
    #[must_use]
    pub fn order_key(&self) -> LexicalRowOrderKey<'_> {
        LexicalRowOrderKey {
            score: self.score,
            repo_relative_path: self.repo_relative_path.as_str(),
            start_line: self.start_line,
            end_line: self.end_line,
            candidate_id: self.candidate_id.as_str(),
        }
    }
}

/// The position of one ranked lexical row, in the generation whose ranking
/// placed it there.
///
/// Equality compares the score by its bits, so it is reflexive for every
/// value and the cursor is `Eq`.
#[derive(Clone, Debug)]
pub struct LexicalCursor {
    /// The generation the page was cut from; only its scores compare.
    pub manifest_generation: ManifestGeneration,
    /// The row's score, finite.
    pub score: f32,
    pub repo_relative_path: RepoRelativePath,
    pub start_line: u32,
    pub end_line: u32,
    pub candidate_id: String,
}

impl PartialEq for LexicalCursor {
    fn eq(&self, other: &Self) -> bool {
        self.manifest_generation == other.manifest_generation
            && self.score.to_bits() == other.score.to_bits()
            && self.repo_relative_path == other.repo_relative_path
            && self.start_line == other.start_line
            && self.end_line == other.end_line
            && self.candidate_id == other.candidate_id
    }
}

impl Eq for LexicalCursor {}

impl LexicalCursor {
    /// The cursor naming `key` in `manifest_generation`.
    #[must_use]
    pub fn at(manifest_generation: ManifestGeneration, key: LexicalRowOrderKey<'_>) -> Self {
        Self {
            manifest_generation,
            score: key.score,
            repo_relative_path: RepoRelativePath::new(key.repo_relative_path),
            start_line: key.start_line,
            end_line: key.end_line,
            candidate_id: key.candidate_id.to_string(),
        }
    }

    /// The boundary's position in the page order.
    #[must_use]
    pub fn order_key(&self) -> LexicalRowOrderKey<'_> {
        LexicalRowOrderKey {
            score: self.score,
            repo_relative_path: self.repo_relative_path.as_str(),
            start_line: self.start_line,
            end_line: self.end_line,
            candidate_id: self.candidate_id.as_str(),
        }
    }

    /// Whether `key` lies strictly after this boundary.
    #[must_use]
    pub fn admits(&self, key: &LexicalRowOrderKey<'_>) -> bool {
        self.order_key().order(key) == Ordering::Less
    }
}

/// Check one ranked page's continuation against its rows.
///
/// The rows are in strict page order, and the page continues exactly when
/// its window says more rows exist: the cursor then names the page's last
/// row in the page's generation. A page that says more exist without a
/// cursor, or names a cursor it does not end on, would make a client skip
/// or repeat rows.
pub fn validate_lexical_page_v1<'a>(
    window: &QueryResultWindowV1,
    rows: impl IntoIterator<Item = LexicalRowOrderKey<'a>>,
    manifest_generation: ManifestGeneration,
    next_cursor: Option<&LexicalCursor>,
) -> Result<(), &'static str> {
    let mut last: Option<LexicalRowOrderKey<'a>> = None;
    for row in rows {
        if !row.score.is_finite() {
            return Err("a ranked lexical row carries a non-finite score");
        }
        if last.is_some_and(|previous| previous.order(&row) != Ordering::Less) {
            return Err("ranked lexical rows are not in page order");
        }
        last = Some(row);
    }
    match (window.has_more(), next_cursor, last) {
        (false, None, _) => Ok(()),
        (false, Some(_), _) => Err("a page with no more rows carries a continuation cursor"),
        (true, None, _) => Err("a page with more rows carries no continuation cursor"),
        (true, Some(_), None) => Err("an empty page carries a continuation cursor"),
        (true, Some(cursor), Some(last)) => {
            if cursor.manifest_generation != manifest_generation {
                return Err("the continuation cursor names another generation than the page");
            }
            if cursor.order_key().order(&last) != Ordering::Equal {
                return Err("the continuation cursor does not name the page's last row");
            }
            Ok(())
        }
    }
}

const LEXICAL_CURSOR_V1_FIELDS: &[&str] = &[
    "manifest_generation",
    "score",
    "repo_relative_path",
    "start_line",
    "end_line",
    "candidate_id",
];

impl Serialize for LexicalCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if !self.score.is_finite() {
            return Err(serde::ser::Error::custom(
                "a lexical cursor score must be finite",
            ));
        }
        let mut state = serializer.serialize_struct("LexicalCursor", 6)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.end()
    }
}

struct LexicalCursorV1Visitor;

impl<'de> Visitor<'de> for LexicalCursorV1Visitor {
    type Value = LexicalCursor;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalCursor map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut score: Option<f32> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        let mut candidate_id: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                "score" => {
                    if score.is_some() {
                        return Err(de::Error::duplicate_field("score"));
                    }
                    score = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "start_line" => {
                    if start_line.is_some() {
                        return Err(de::Error::duplicate_field("start_line"));
                    }
                    start_line = Some(map.next_value()?);
                }
                "end_line" => {
                    if end_line.is_some() {
                        return Err(de::Error::duplicate_field("end_line"));
                    }
                    end_line = Some(map.next_value()?);
                }
                "candidate_id" => {
                    if candidate_id.is_some() {
                        return Err(de::Error::duplicate_field("candidate_id"));
                    }
                    candidate_id = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, LEXICAL_CURSOR_V1_FIELDS));
                }
            }
        }
        let score = score.ok_or_else(|| de::Error::missing_field("score"))?;
        if !score.is_finite() {
            return Err(de::Error::custom("a lexical cursor score must be finite"));
        }
        Ok(LexicalCursor {
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            score,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalCursor",
            LEXICAL_CURSOR_V1_FIELDS,
            LexicalCursorV1Visitor,
        )
    }
}
