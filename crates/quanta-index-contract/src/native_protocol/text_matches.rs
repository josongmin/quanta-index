//! Native text matches: every match of one byte-exact literal or regex over
//! the selected immutable sources, as stable `(repository, path, byte)` rows.
//!
//! # Match semantics
//!
//! The rows of one query over one selected view are exactly the matches that
//! ripgrep's default (line-oriented, non-multiline) search reports:
//!
//! - Only sources admitted as UTF-8 text by the selected view are searched.
//! - Each source's original bytes are split into lines at `\n`; the
//!   terminator is not part of any line. A match never spans a line, and a
//!   pattern that can only match by containing `\n` is refused.
//! - Within a line, matches are leftmost-first and non-overlapping, in the
//!   iteration order of `regex::bytes::Regex::find_iter`. Empty matches are
//!   never reported, so every row covers a nonempty byte range.
//! - A [`NativeTextMatchPattern::Literal`] is the regex of its escaped text.
//!   A [`NativeTextMatchPattern::Regex`] uses the Rust `regex` syntax in
//!   Unicode mode, ripgrep's default engine. Byte-level constructs that could
//!   split a UTF-8 sequence are refused, so every range is UTF-8 aligned.
//! - The query's case policy decides folding: [`NativeTextMatchCase::Sensitive`]
//!   compares the original bytes exactly, and [`NativeTextMatchCase::Insensitive`]
//!   applies Unicode simple case folding, ripgrep's `-i`.
//! - The query's constraints restrict the searched files before matching,
//!   with the same meaning as for ranked native text queries.
//!
//! # Order, pages and refusals
//!
//! Rows are ordered by `(repository, path, start)`; the unscoped repository
//! slot sorts first. One `(repository, path)` names one file of the selected
//! view, so its rows share one Source identity and do not overlap.
//!
//! A page is [`NativeTextMatchCompletion::More`] or
//! [`NativeTextMatchCompletion::Exhausted`]. Running out of the request budget
//! and cancellation are refusals of the whole page, never a shorter page that
//! claims completion: `Exhausted` asserts that no match follows the page.

use crate::bounded_cluster_members::BoundedVecV1;
use crate::index::{Digest, IndexInstanceId, ViewRef, ViewSelection};
use crate::{NativeSourceIdentity, PreviewByteRange, QueryConstraintSetV1, RepoId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
use std::cmp::Ordering;

/// Upper bound of [`NativeTextMatchesRequest::limit`] and of one page's rows.
pub const MAX_NATIVE_TEXT_MATCH_LIMIT: u32 = 1_024;
const MAX_NATIVE_TEXT_MATCH_ROWS: usize = 1_024;
/// Upper bound of a literal or regex pattern in UTF-8 bytes.
pub const MAX_NATIVE_TEXT_MATCH_PATTERN_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeTextMatchPattern {
    /// Byte-exact text, matched as the regex of its escaped form.
    Literal(String),
    /// Rust `regex` syntax in Unicode mode, ripgrep's default engine.
    Regex(String),
}
quanta_index_contract_base::native_serde_enum!(NativeTextMatchPattern {
    units {}
    newtypes { Literal = 0 => String, Regex = 1 => String }
    records {}
});

/// The query's case policy; see the module documentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextMatchCase {
    Sensitive,
    Insensitive,
}
quanta_index_contract_base::native_serde_enum!(NativeTextMatchCase {
    units { Sensitive = 0, Insensitive = 1 }
    newtypes {}
    records {}
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextMatchQuery {
    pub pattern: NativeTextMatchPattern,
    pub case: NativeTextMatchCase,
    pub constraints: QueryConstraintSetV1,
}
quanta_index_contract_base::native_serde_struct!(NativeTextMatchQuery {
    pattern: NativeTextMatchPattern,
    case: NativeTextMatchCase,
    constraints: QueryConstraintSetV1,
});

impl NativeTextMatchQuery {
    /// Shape checks only. Compiling the regex and refusing patterns that
    /// need `\n` or split UTF-8 belong to the engine.
    pub fn validate(&self) -> Result<(), &'static str> {
        let text = match &self.pattern {
            NativeTextMatchPattern::Literal(text) | NativeTextMatchPattern::Regex(text) => text,
        };
        if text.is_empty() || text.len() > MAX_NATIVE_TEXT_MATCH_PATTERN_BYTES {
            return Err("native text match pattern must contain 1..=4096 UTF-8 bytes");
        }
        Ok(())
    }
}

/// The digest a match cursor binds; it covers the instance and the query.
pub fn native_text_match_query_digest(
    instance: IndexInstanceId,
    query: &NativeTextMatchQuery,
) -> Result<Digest, String> {
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    writer
        .0
        .update(b"quanta-index/native-text-matches/query/v1\0");
    writer.0.update(instance.0);
    serde_json::to_writer(&mut writer, query).map_err(|error| error.to_string())?;
    Ok(Digest(writer.0.finalize().into()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextMatch {
    /// Authenticated repository; `None` is the unscoped repository slot.
    pub repository: Option<RepoId>,
    pub path: String,
    /// The Native Source record of the matched file in the selected view.
    pub native_source: NativeSourceIdentity,
    /// Half-open, nonempty byte range in the selected immutable source.
    pub range: PreviewByteRange,
}
quanta_index_contract_base::native_serde_struct!(NativeTextMatch {
    repository: Option<RepoId>,
    path: String,
    native_source: NativeSourceIdentity,
    range: PreviewByteRange,
});

/// Stateless continuation anchor: the last row of the page it ends.
///
/// It names the whole Source identity of that row's file, so a continuation
/// whose file now has another owner or another Source is refused rather than
/// resumed at a byte offset of a different text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextMatchCursor {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub query: Digest,
    pub repository: Option<RepoId>,
    pub path: String,
    pub native_source: NativeSourceIdentity,
    pub start: u64,
}
quanta_index_contract_base::native_serde_struct!(NativeTextMatchCursor {
    view: ViewRef,
    instance: IndexInstanceId,
    query: Digest,
    repository: Option<RepoId>,
    path: String,
    native_source: NativeSourceIdentity,
    start: u64,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextMatchCompletion {
    /// The page holds exactly `limit` rows and `next` continues after them.
    More,
    /// No match of the selected view follows this page.
    Exhausted,
}
quanta_index_contract_base::native_serde_enum!(NativeTextMatchCompletion {
    units { More = 0, Exhausted = 1 }
    newtypes {}
    records {}
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextMatchesRequest {
    pub selection: ViewSelection,
    pub instance: IndexInstanceId,
    pub query: NativeTextMatchQuery,
    pub after: Option<NativeTextMatchCursor>,
    /// Rows per page, `1..=MAX_NATIVE_TEXT_MATCH_LIMIT`.
    pub limit: u32,
}

/// The decoded fields before [`NativeTextMatchesRequest::validate`] admits them.
struct NativeTextMatchesRequestWire {
    selection: ViewSelection,
    instance: IndexInstanceId,
    query: NativeTextMatchQuery,
    after: Option<NativeTextMatchCursor>,
    limit: u32,
}
quanta_index_contract_base::native_serde_struct!(NativeTextMatchesRequestWire {
    selection: ViewSelection,
    instance: IndexInstanceId,
    query: NativeTextMatchQuery,
    after: Option<NativeTextMatchCursor>,
    limit: u32,
});

/// Encoding and decoding both refuse a request that `validate` refuses.
impl Serialize for NativeTextMatchesRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("NativeTextMatchesRequestWire", 5)?;
        state.serialize_field("selection", &self.selection)?;
        state.serialize_field("instance", &self.instance)?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("after", &self.after)?;
        state.serialize_field("limit", &self.limit)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for NativeTextMatchesRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = NativeTextMatchesRequestWire::deserialize(deserializer)?;
        let request = Self {
            selection: wire.selection,
            instance: wire.instance,
            query: wire.query,
            after: wire.after,
            limit: wire.limit,
        };
        request.validate().map_err(serde::de::Error::custom)?;
        Ok(request)
    }
}

impl NativeTextMatchesRequest {
    /// A continuation must name the exact view, instance and query that
    /// produced its cursor.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_NATIVE_TEXT_MATCH_LIMIT).contains(&self.limit) {
            return Err("native text match limit must be 1..=1024".into());
        }
        self.query.validate()?;
        if let Some(cursor) = &self.after {
            validate_cursor_key(cursor)?;
            if self.selection != ViewSelection::Exact(cursor.view) {
                return Err("native text match cursor requires its exact view".into());
            }
            if cursor.instance != self.instance {
                return Err("native text match cursor names another instance".into());
            }
            if cursor.query != native_text_match_query_digest(self.instance, &self.query)? {
                return Err("native text match cursor names another query".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextMatchPage {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub query: Digest,
    pub matches: Vec<NativeTextMatch>,
    pub completion: NativeTextMatchCompletion,
    pub next: Option<NativeTextMatchCursor>,
}

impl NativeTextMatchPage {
    /// Checks that need no request: row shape, order, per-file identity and
    /// non-overlap, and the completion/cursor pairing.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.matches.len() > MAX_NATIVE_TEXT_MATCH_ROWS {
            return Err("native text match page exceeds 1024 rows");
        }
        let mut previous: Option<&NativeTextMatch> = None;
        for row in &self.matches {
            row.range.validate()?;
            if !crate::index::is_canonical_source_path(&row.path) {
                return Err("native text match path is not a canonical source path");
            }
            if let Some(prior) = previous {
                match compare_match_key(prior, row) {
                    Ordering::Less => {}
                    Ordering::Equal | Ordering::Greater => {
                        return Err(
                            "native text matches must strictly ascend by repository, path and start",
                        );
                    }
                }
                if prior.repository == row.repository && prior.path == row.path {
                    if prior.native_source != row.native_source {
                        return Err("native text matches of one file disagree on its source");
                    }
                    if row.range.start < prior.range.end {
                        return Err("native text matches of one file must not overlap");
                    }
                }
            }
            previous = Some(row);
        }
        match (self.completion, &self.next, previous) {
            (NativeTextMatchCompletion::Exhausted, None, _) => Ok(()),
            (NativeTextMatchCompletion::More, Some(next), Some(last)) => {
                if next.view != self.view
                    || next.instance != self.instance
                    || next.query != self.query
                    || next.repository != last.repository
                    || next.path != last.path
                    || next.native_source != last.native_source
                    || next.start != last.range.start
                {
                    return Err("native text match cursor differs from the last row");
                }
                Ok(())
            }
            (NativeTextMatchCompletion::More, _, _) => {
                Err("native text match continuation needs rows and a cursor")
            }
            (NativeTextMatchCompletion::Exhausted, Some(_), _) => {
                Err("an exhausted native text match page has no cursor")
            }
        }
    }

    /// Checks the page against the request it answers.
    pub fn validate_for(&self, request: &NativeTextMatchesRequest) -> Result<(), String> {
        request.validate()?;
        self.validate()?;
        if self.instance != request.instance
            || self.query != native_text_match_query_digest(request.instance, &request.query)?
        {
            return Err("native text match page answers another instance or query".into());
        }
        match request.selection {
            ViewSelection::Exact(view) if view != self.view => {
                return Err("native text match page answers another view".into());
            }
            ViewSelection::Latest { workspace, epoch }
                if workspace != self.view.workspace || epoch != self.view.epoch =>
            {
                return Err("native text match page answers another workspace".into());
            }
            ViewSelection::Exact(_) | ViewSelection::Latest { .. } => {}
        }
        let rows = u32::try_from(self.matches.len())
            .map_err(|_width| "native text match row count overflows u32")?;
        if rows > request.limit
            || (self.completion == NativeTextMatchCompletion::More && rows != request.limit)
        {
            return Err("native text match page disagrees with the request limit".into());
        }
        if let (Some(cursor), Some(first)) = (&request.after, self.matches.first())
            && compare_cursor_key(cursor, first) != Ordering::Less
        {
            return Err("native text match page does not continue after its cursor".into());
        }
        Ok(())
    }
}

struct NativeTextMatchPageWire {
    view: ViewRef,
    instance: IndexInstanceId,
    query: Digest,
    matches: BoundedVecV1<NativeTextMatch, MAX_NATIVE_TEXT_MATCH_ROWS>,
    completion: NativeTextMatchCompletion,
    next: Option<NativeTextMatchCursor>,
}
quanta_index_contract_base::native_serde_struct!(NativeTextMatchPageWire {
    view: ViewRef,
    instance: IndexInstanceId,
    query: Digest,
    matches: BoundedVecV1<NativeTextMatch, MAX_NATIVE_TEXT_MATCH_ROWS>,
    completion: NativeTextMatchCompletion,
    next: Option<NativeTextMatchCursor>,
});

impl Serialize for NativeTextMatchPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("NativeTextMatchPageWire", 6)?;
        state.serialize_field("view", &self.view)?;
        state.serialize_field("instance", &self.instance)?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("matches", &self.matches)?;
        state.serialize_field("completion", &self.completion)?;
        state.serialize_field("next", &self.next)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for NativeTextMatchPage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = NativeTextMatchPageWire::deserialize(deserializer)?;
        let page = Self {
            view: wire.view,
            instance: wire.instance,
            query: wire.query,
            matches: wire.matches.into_inner(),
            completion: wire.completion,
            next: wire.next,
        };
        page.validate().map_err(serde::de::Error::custom)?;
        Ok(page)
    }
}

/// The deterministic row order: repository (unscoped first), path, start.
fn compare_match_key(left: &NativeTextMatch, right: &NativeTextMatch) -> Ordering {
    left.repository
        .cmp(&right.repository)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.range.start.cmp(&right.range.start))
}

fn compare_cursor_key(cursor: &NativeTextMatchCursor, row: &NativeTextMatch) -> Ordering {
    cursor
        .repository
        .cmp(&row.repository)
        .then_with(|| cursor.path.cmp(&row.path))
        .then_with(|| cursor.start.cmp(&row.range.start))
}

fn validate_cursor_key(cursor: &NativeTextMatchCursor) -> Result<(), &'static str> {
    if crate::index::is_canonical_source_path(&cursor.path) {
        Ok(())
    } else {
        Err("native text match cursor path is not a canonical source path")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        CollectionId, ObjectRef, OwnerKey, ProducerId, SlotId, StableOwnerId, TenantId,
        WorkspaceKey, WorktreeId,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A failed check is a test error, not a panic.
    macro_rules! ensure {
        ($check:expr) => {
            if !$check {
                return Err(concat!("check failed: ", stringify!($check)).into());
            }
        };
    }

    fn at<'a>(
        wire: &'a serde_json::Value,
        pointer: &str,
    ) -> Result<&'a serde_json::Value, Box<dyn std::error::Error>> {
        Ok(wire
            .pointer(pointer)
            .ok_or("fixed wire pointer is absent")?)
    }

    fn set(wire: &mut serde_json::Value, pointer: &str, value: serde_json::Value) -> TestResult {
        *wire
            .pointer_mut(pointer)
            .ok_or("fixed wire pointer is absent")? = value;
        Ok(())
    }

    fn view() -> ViewRef {
        ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 9,
            digest: Digest([5; 32]),
        }
    }

    fn instance() -> IndexInstanceId {
        IndexInstanceId([8; 16])
    }

    fn query() -> NativeTextMatchQuery {
        NativeTextMatchQuery {
            pattern: NativeTextMatchPattern::Regex("fn \\w+".into()),
            case: NativeTextMatchCase::Sensitive,
            constraints: QueryConstraintSetV1::unconstrained(),
        }
    }

    fn source(id: u8) -> NativeSourceIdentity {
        NativeSourceIdentity {
            owner: OwnerKey {
                producer: ProducerId([11; 16]),
                stable_id: StableOwnerId([id; 16]),
            },
            source: ObjectRef(Digest([id; 32])),
        }
    }

    fn row(repository: Option<&str>, path: &str, id: u8, start: u64, end: u64) -> TestResultRow {
        Ok(NativeTextMatch {
            repository: repository.map(RepoId::try_from).transpose()?,
            path: path.into(),
            native_source: source(id),
            range: PreviewByteRange { start, end },
        })
    }
    type TestResultRow = Result<NativeTextMatch, Box<dyn std::error::Error>>;

    fn cursor_after(page_row: &NativeTextMatch) -> Result<NativeTextMatchCursor, String> {
        Ok(NativeTextMatchCursor {
            view: view(),
            instance: instance(),
            query: native_text_match_query_digest(instance(), &query())?,
            repository: page_row.repository.clone(),
            path: page_row.path.clone(),
            native_source: page_row.native_source,
            start: page_row.range.start,
        })
    }

    /// Unscoped first, then repositories; paths and starts ascend.
    fn rows() -> Result<Vec<NativeTextMatch>, Box<dyn std::error::Error>> {
        Ok(vec![
            row(None, "src/z.rs", 1, 0, 6)?,
            row(Some("corp-a"), "src/a.rs", 2, 4, 9)?,
            row(Some("corp-a"), "src/a.rs", 2, 9, 12)?,
            row(Some("corp-b"), "src/a.rs", 3, 0, 4)?,
        ])
    }

    fn page(
        completion: NativeTextMatchCompletion,
    ) -> Result<NativeTextMatchPage, Box<dyn std::error::Error>> {
        let matches = rows()?;
        let next = match completion {
            NativeTextMatchCompletion::More => {
                Some(cursor_after(matches.last().ok_or("fixed rows are empty")?)?)
            }
            NativeTextMatchCompletion::Exhausted => None,
        };
        Ok(NativeTextMatchPage {
            view: view(),
            instance: instance(),
            query: native_text_match_query_digest(instance(), &query())?,
            matches,
            completion,
            next,
        })
    }

    fn request(after: Option<NativeTextMatchCursor>, limit: u32) -> NativeTextMatchesRequest {
        NativeTextMatchesRequest {
            selection: ViewSelection::Exact(view()),
            instance: instance(),
            query: query(),
            after,
            limit,
        }
    }

    fn json_and_cbor_roundtrip<T>(value: &T) -> TestResult
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value)?;
        ensure!(&serde_json::from_str::<T>(&json)? == value);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(value, &mut cbor)?;
        ensure!(&ciborium::de::from_reader::<T, _>(cbor.as_slice())? == value);
        Ok(())
    }

    fn decode_refused<T>(wire: &serde_json::Value) -> TestResult
    where
        T: for<'de> Deserialize<'de> + std::fmt::Debug,
    {
        ensure!(serde_json::from_value::<T>(wire.clone()).is_err());
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(wire, &mut cbor)?;
        ensure!(ciborium::de::from_reader::<T, _>(cbor.as_slice()).is_err());
        Ok(())
    }

    #[test]
    fn match_request_and_pages_roundtrip_in_json_and_cbor() -> TestResult {
        let exhausted = page(NativeTextMatchCompletion::Exhausted)?;
        let more = page(NativeTextMatchCompletion::More)?;
        let continued = request(more.next.clone(), 4);
        continued.validate()?;
        exhausted.validate_for(&request(None, 4))?;
        more.validate_for(&request(None, 4))?;
        json_and_cbor_roundtrip(&exhausted)?;
        json_and_cbor_roundtrip(&more)?;
        json_and_cbor_roundtrip(&continued)?;
        json_and_cbor_roundtrip(&NativeTextMatchQuery {
            pattern: NativeTextMatchPattern::Literal("a.b".into()),
            case: NativeTextMatchCase::Insensitive,
            constraints: QueryConstraintSetV1::unconstrained(),
        })?;
        Ok(())
    }

    #[test]
    fn match_wire_is_the_documented_closed_shape() -> TestResult {
        let wire = serde_json::to_value(page(NativeTextMatchCompletion::Exhausted)?)?;
        ensure!(at(&wire, "/completion")? == "Exhausted");
        ensure!(at(&wire, "/next")?.is_null());
        ensure!(at(&wire, "/matches/0/repository")?.is_null());
        ensure!(at(&wire, "/matches/1/repository")? == "corp-a");
        ensure!(at(&wire, "/matches/1/range")? == &serde_json::json!({"start": 4, "end": 9}));
        ensure!(at(&wire, "/matches/1/native_source/owner")?.is_object());
        let mut unknown = wire.clone();
        let _replaced = unknown
            .as_object_mut()
            .ok_or("fixed page wire is not an object")?
            .insert("partial".into(), serde_json::Value::Bool(true));
        decode_refused::<NativeTextMatchPage>(&unknown)?;
        let mut partial = wire;
        set(&mut partial, "/completion", serde_json::json!("Partial"))?;
        decode_refused::<NativeTextMatchPage>(&partial)
    }

    #[test]
    fn match_rows_refuse_empty_reversed_and_unordered_ranges() -> TestResult {
        let base = serde_json::to_value(page(NativeTextMatchCompletion::Exhausted)?)?;
        let cases = [
            // Empty and reversed byte ranges.
            (
                "/matches/0/range",
                serde_json::json!({"start": 3, "end": 3}),
            ),
            (
                "/matches/0/range",
                serde_json::json!({"start": 6, "end": 1}),
            ),
            // Rows out of (repository, path, start) order.
            ("/matches/1/repository", serde_json::Value::Null),
            (
                "/matches/2/range",
                serde_json::json!({"start": 2, "end": 3}),
            ),
            ("/matches/3/repository", serde_json::json!("corp-0")),
            // A duplicate start and an overlap inside one file.
            (
                "/matches/2/range",
                serde_json::json!({"start": 4, "end": 9}),
            ),
            (
                "/matches/2/range",
                serde_json::json!({"start": 8, "end": 12}),
            ),
            // One file must keep one Source identity.
            (
                "/matches/2/native_source/source",
                serde_json::json!(vec![77_u8; 32]),
            ),
            // Paths are canonical relative source paths.
            ("/matches/0/path", serde_json::json!("/abs.rs")),
            ("/matches/0/path", serde_json::json!("src/../a.rs")),
            ("/matches/0/path", serde_json::json!("")),
        ];
        for (pointer, value) in cases {
            let mut wire = base.clone();
            set(&mut wire, pointer, value)?;
            decode_refused::<NativeTextMatchPage>(&wire)?;
        }
        // Adjacent, non-overlapping matches in one file are valid.
        let mut adjacent = page(NativeTextMatchCompletion::Exhausted)?;
        adjacent.validate()?;
        let first = adjacent.matches.get_mut(1).ok_or("fixed row is absent")?;
        first.range = PreviewByteRange { start: 0, end: 9 };
        adjacent.validate()?;
        Ok(())
    }

    #[test]
    fn completion_and_cursor_must_agree_with_the_last_row() -> TestResult {
        // More without a cursor, Exhausted with one, and More without rows.
        let mut more = page(NativeTextMatchCompletion::More)?;
        more.next = None;
        ensure!(more.validate().is_err());
        let mut exhausted = page(NativeTextMatchCompletion::Exhausted)?;
        exhausted.next = page(NativeTextMatchCompletion::More)?.next;
        ensure!(exhausted.validate().is_err());
        let mut empty_more = page(NativeTextMatchCompletion::More)?;
        empty_more.matches.clear();
        ensure!(empty_more.validate().is_err());
        // An empty exhausted page is the complete answer of a matchless query.
        let mut empty = page(NativeTextMatchCompletion::Exhausted)?;
        empty.matches.clear();
        empty.validate()?;

        let good = page(NativeTextMatchCompletion::More)?;
        let mutations: [fn(&mut NativeTextMatchCursor); 8] = [
            |cursor| cursor.view.sequence = 10,
            |cursor| cursor.instance = IndexInstanceId([9; 16]),
            |cursor| cursor.query = Digest([0; 32]),
            |cursor| cursor.repository = None,
            |cursor| cursor.path = "src/b.rs".into(),
            |cursor| cursor.native_source.owner.stable_id = StableOwnerId([99; 16]),
            |cursor| cursor.native_source.source = ObjectRef(Digest([99; 32])),
            |cursor| cursor.start = 1,
        ];
        for mutate in mutations {
            let mut forged = good.clone();
            mutate(forged.next.as_mut().ok_or("fixed cursor is absent")?);
            ensure!(forged.validate().is_err());
            let mut wire = serde_json::to_value(&good)?;
            set(
                &mut wire,
                "/next",
                serde_json::to_value(forged.next.as_ref())?,
            )?;
            decode_refused::<NativeTextMatchPage>(&wire)?;
        }
        Ok(())
    }

    #[test]
    fn cursor_binds_view_instance_and_query() -> TestResult {
        let last = rows()?.pop().ok_or("fixed rows are empty")?;
        let cursor = cursor_after(&last)?;
        request(Some(cursor.clone()), 2).validate()?;
        // Another view, a Latest selection, another instance and another query.
        let mut other_view = request(Some(cursor.clone()), 2);
        other_view.selection = ViewSelection::Exact(ViewRef {
            sequence: 10,
            ..view()
        });
        ensure!(other_view.validate().is_err());
        let mut latest = request(Some(cursor.clone()), 2);
        latest.selection = ViewSelection::Latest {
            workspace: view().workspace,
            epoch: 1,
        };
        ensure!(latest.validate().is_err());
        let mut other_instance = request(Some(cursor.clone()), 2);
        other_instance.instance = IndexInstanceId([9; 16]);
        ensure!(other_instance.validate().is_err());
        for changed in [
            NativeTextMatchQuery {
                pattern: NativeTextMatchPattern::Literal("fn \\w+".into()),
                ..query()
            },
            NativeTextMatchQuery {
                case: NativeTextMatchCase::Insensitive,
                ..query()
            },
            NativeTextMatchQuery {
                constraints: QueryConstraintSetV1::from_exact_repo_relative_path(
                    crate::ExactRepoRelativePathV1::new("src/a.rs")?,
                ),
                ..query()
            },
        ] {
            let mut other_query = request(Some(cursor.clone()), 2);
            other_query.query = changed;
            ensure!(other_query.validate().is_err());
        }
        // The digest is a stable function of instance and query.
        ensure!(
            native_text_match_query_digest(instance(), &query())?
                == native_text_match_query_digest(instance(), &query())?
        );
        ensure!(
            native_text_match_query_digest(instance(), &query())?
                != native_text_match_query_digest(IndexInstanceId([9; 16]), &query())?
        );
        Ok(())
    }

    #[test]
    fn page_must_continue_strictly_after_its_request_cursor() -> TestResult {
        let matches = rows()?;
        let anchor = matches.get(1).ok_or("fixed row is absent")?;
        let tail = NativeTextMatchPage {
            matches: matches.get(2..).ok_or("fixed rows are absent")?.to_vec(),
            ..page(NativeTextMatchCompletion::Exhausted)?
        };
        tail.validate_for(&request(Some(cursor_after(anchor)?), 4))?;
        // Repeating the anchor row, or restarting before it, is refused.
        let repeated = NativeTextMatchPage {
            matches: matches.get(1..).ok_or("fixed rows are absent")?.to_vec(),
            ..tail
        };
        ensure!(
            repeated
                .validate_for(&request(Some(cursor_after(anchor)?), 4))
                .is_err()
        );
        let restarted = page(NativeTextMatchCompletion::Exhausted)?;
        ensure!(
            restarted
                .validate_for(&request(Some(cursor_after(anchor)?), 4))
                .is_err()
        );
        // The page answers its own view, instance and query.
        let foreign = NativeTextMatchPage {
            view: ViewRef {
                sequence: 10,
                ..view()
            },
            ..page(NativeTextMatchCompletion::Exhausted)?
        };
        ensure!(foreign.validate_for(&request(None, 4)).is_err());
        let latest = NativeTextMatchesRequest {
            selection: ViewSelection::Latest {
                workspace: view().workspace,
                epoch: 2,
            },
            ..request(None, 4)
        };
        ensure!(
            page(NativeTextMatchCompletion::Exhausted)?
                .validate_for(&latest)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn limits_bound_requests_patterns_and_pages() -> TestResult {
        ensure!(request(None, 0).validate().is_err());
        ensure!(
            request(None, MAX_NATIVE_TEXT_MATCH_LIMIT + 1)
                .validate()
                .is_err()
        );
        request(None, MAX_NATIVE_TEXT_MATCH_LIMIT).validate()?;
        for pattern in [
            NativeTextMatchPattern::Literal(String::new()),
            NativeTextMatchPattern::Regex("x".repeat(MAX_NATIVE_TEXT_MATCH_PATTERN_BYTES + 1)),
        ] {
            let mut oversized = request(None, 1);
            oversized.query.pattern = pattern;
            ensure!(oversized.validate().is_err());
        }
        // More must fill its limit; Exhausted may be shorter; no page exceeds it.
        let more = page(NativeTextMatchCompletion::More)?;
        more.validate_for(&request(None, 4))?;
        ensure!(more.validate_for(&request(None, 5)).is_err());
        ensure!(more.validate_for(&request(None, 3)).is_err());
        page(NativeTextMatchCompletion::Exhausted)?.validate_for(&request(None, 5))?;
        // A page above the row bound is refused at decode.
        let mut wire = serde_json::to_value(page(NativeTextMatchCompletion::Exhausted)?)?;
        let first = at(&wire, "/matches/0")?.clone();
        set(
            &mut wire,
            "/matches",
            serde_json::Value::Array(vec![first; MAX_NATIVE_TEXT_MATCH_ROWS + 1]),
        )?;
        decode_refused::<NativeTextMatchPage>(&wire)
    }

    #[test]
    fn request_decode_refuses_what_validate_refuses() -> TestResult {
        let last = rows()?.pop().ok_or("fixed rows are empty")?;
        let valid = serde_json::to_value(request(Some(cursor_after(&last)?), 2))?;
        let _decoded: NativeTextMatchesRequest = serde_json::from_value(valid.clone())?;
        let forged = [
            ("/limit", serde_json::json!(0)),
            ("/limit", serde_json::json!(MAX_NATIVE_TEXT_MATCH_LIMIT + 1)),
            ("/query/pattern", serde_json::json!({"Literal": ""})),
            ("/instance", serde_json::json!(vec![9_u8; 16])),
            ("/after/query", serde_json::json!(vec![0_u8; 32])),
            ("/after/path", serde_json::json!("../escape.rs")),
            (
                "/selection",
                serde_json::json!({"Latest": {
                "workspace": at(&valid, "/after/view/workspace")?.clone(), "epoch": 1}}),
            ),
        ];
        for (pointer, value) in forged {
            let mut wire = valid.clone();
            set(&mut wire, pointer, value)?;
            decode_refused::<NativeTextMatchesRequest>(&wire)?;
        }
        Ok(())
    }

    /// The envelope tags are fixed public wire; expected keys are literal.
    #[test]
    fn native_envelopes_carry_match_requests_and_pages() -> TestResult {
        use crate::native_protocol::{
            NativeIndexRequest, NativeIndexResponse, NativeReadSessionId, NativeReadSessionRequest,
        };
        struct Borrowed<'a>(&'a NativeTextMatchPage);
        impl Serialize for Borrowed<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_text_match_page_ref(self.0, serializer)
            }
        }
        let continued = request(page(NativeTextMatchCompletion::More)?.next, 4);
        let request_wire = serde_json::to_value(&continued)?;
        let stateless = NativeIndexRequest::TextMatches(continued.clone());
        ensure!(
            serde_json::to_value(&stateless)? == serde_json::json!({"TextMatches": request_wire})
        );
        let session =
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextMatches {
                session: NativeReadSessionId([7; 32]),
                request: continued,
            });
        ensure!(
            serde_json::to_value(&session)?
                == serde_json::json!({"ReadSession": {"SearchTextMatches": {
                    "session": vec![7_u8; 32], "request": request_wire}}})
        );
        json_and_cbor_roundtrip(&stateless)?;
        json_and_cbor_roundtrip(&session)?;
        let reply = page(NativeTextMatchCompletion::More)?;
        let reply_wire = serde_json::to_value(&reply)?;
        let response = NativeIndexResponse::TextMatchPage(reply.clone());
        ensure!(
            serde_json::to_value(&response)? == serde_json::json!({"TextMatchPage": reply_wire})
        );
        ensure!(serde_json::to_vec(&Borrowed(&reply))? == serde_json::to_vec(&response)?);
        json_and_cbor_roundtrip(&response)?;
        // The envelope decodes the request through its validating codec.
        let mut forged = request_wire;
        set(&mut forged, "/limit", serde_json::json!(0))?;
        decode_refused::<NativeIndexRequest>(&serde_json::json!({"TextMatches": forged}))
    }
}
