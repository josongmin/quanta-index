//! Native `select:path` returns one ranked representative per repository path.

use super::read_session::{NativeReadSessionId, NativeTextPathCursorV1};
use super::{NativeTextPreview, NativeTextResult};
use crate::index::IndexKind;
use crate::{PreviewByteRange, RepoId};

/// The kind of the product Native Text adapter.
///
/// Product composition registers its Text member under exactly this kind, so
/// a client finds the default code-search member of a captured profile by
/// descriptor identity rather than by configuration or name.
pub const NATIVE_TEXT_KIND: IndexKind = IndexKind([72; 16]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPathRankingV1 {
    /// The representative rows in score, repository, path order. `matched`
    /// counts groups before the result cap, independently of page position.
    pub ranking: NativeTextResult,
    /// Authenticated source repository, one per representative in rank order.
    pub repositories: Vec<RepoId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPathResultV1 {
    pub ranked: NativeTextPathRankingV1,
    pub previews: Option<Vec<NativeTextPreview>>,
}

impl NativeTextPathResultV1 {
    pub fn validate_previews(&self) -> Result<(), &'static str> {
        if let Some(previews) = &self.previews {
            super::text_preview::validate_native_text_previews(&self.ranked.ranking, previews)?;
        }
        Ok(())
    }
}

/// The most bytes on each side of a `CodeSearch` content match that its
/// excerpt shows, before both edges move inward to UTF-8 boundaries.
pub const NATIVE_CODE_SEARCH_CONTEXT_BYTES: u64 = 120;
/// Sources larger than this keep their match lines but carry no excerpt.
pub const MAX_NATIVE_CODE_SEARCH_EXCERPT_SOURCE_BYTES: u64 = 1024 * 1024;

/// The excerpt of a `CodeSearch` content match: `text` is exactly the verified
/// Source bytes `context`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCodeSearchExcerpt {
    pub context: PreviewByteRange,
    pub text: String,
}

/// The representative match of one `CodeSearch` file row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeCodeSearchMatch {
    /// Source bytes `start..end`, the 1-based lines of their first and last
    /// byte, and the excerpt around them; `excerpt` is absent only when the
    /// Source exceeds [`MAX_NATIVE_CODE_SEARCH_EXCERPT_SOURCE_BYTES`].
    Content {
        start: u64,
        end: u64,
        start_line: u32,
        end_line: u32,
        excerpt: Option<NativeCodeSearchExcerpt>,
    },
    /// Repository-relative path bytes `start..end` of the row's path.
    Path { start: u64, end: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPathPageV1 {
    pub session: NativeReadSessionId,
    pub ranked: NativeTextPathRankingV1,
    /// Groups strictly after the input cursor, before the current page cap.
    pub remaining: u64,
    pub after: Option<NativeTextPathCursorV1>,
    /// For a `CodeSearch` query, one entry per ranked row, in row order; a row
    /// without a positive witness has `None`. Empty for other queries.
    pub code_search: Vec<Option<NativeCodeSearchMatch>>,
}

impl NativeTextPathPageV1 {
    /// Check the `CodeSearch` matches against the rows they describe.
    pub fn validate_code_search(&self) -> Result<(), &'static str> {
        let hits = &self.ranked.ranking.hits;
        if self.code_search.is_empty() {
            return Ok(());
        }
        if self.code_search.len() != hits.len() {
            return Err("CodeSearch matches must pair one entry with each row");
        }
        for (hit, matched) in hits.iter().zip(&self.code_search) {
            match matched {
                None => {}
                Some(NativeCodeSearchMatch::Path { start, end }) => {
                    let empty = "CodeSearch path match must be a nonempty range";
                    let from = usize::try_from(*start).map_err(|_width| empty)?;
                    let to = usize::try_from(*end).map_err(|_width| empty)?;
                    if from >= to {
                        return Err(empty);
                    }
                    if hit.path.get(from..to).is_none() {
                        return Err("CodeSearch path match must lie on its row's path characters");
                    }
                }
                Some(NativeCodeSearchMatch::Content {
                    start,
                    end,
                    start_line,
                    end_line,
                    excerpt,
                }) => {
                    if start >= end || *start_line == 0 || start_line > end_line {
                        return Err("CodeSearch content match must have a range and 1-based lines");
                    }
                    if let Some(excerpt) = excerpt {
                        let context = &excerpt.context;
                        let width = context
                            .end
                            .checked_sub(context.start)
                            .ok_or("CodeSearch excerpt context is reversed")?;
                        let reach = end
                            .checked_sub(*start)
                            .and_then(|focus| {
                                NATIVE_CODE_SEARCH_CONTEXT_BYTES
                                    .checked_mul(2)
                                    .and_then(|sides| focus.checked_add(sides))
                            })
                            .ok_or("CodeSearch excerpt width overflows")?;
                        if context.start > *start
                            || context.end < *end
                            || width > reach
                            || u64::try_from(excerpt.text.len()) != Ok(width)
                        {
                            return Err(
                                "CodeSearch excerpt must cover its match within the context bound",
                            );
                        }
                        // The match is a character range of the excerpt text.
                        let misaligned = "CodeSearch content match must lie on excerpt characters";
                        let relative = |offset: u64| -> Result<usize, &'static str> {
                            let relative = offset.checked_sub(context.start).ok_or(misaligned)?;
                            usize::try_from(relative).map_err(|_width| misaligned)
                        };
                        let (from, to) = (relative(*start)?, relative(*end)?);
                        if excerpt.text.get(from..to).is_none() {
                            return Err(misaligned);
                        }
                        // Lines count newlines on the verified bytes: the
                        // match's first and last byte lie `end_line -
                        // start_line` newlines apart, and a context that
                        // starts the Source fixes the first line.
                        let lines = "CodeSearch content match lines differ from its excerpt";
                        let newlines =
                            |range: std::ops::Range<usize>| -> Result<u64, &'static str> {
                                let bytes = excerpt.text.as_bytes().get(range).ok_or(lines)?;
                                #[expect(
                                    clippy::naive_bytecount,
                                    reason = "an excerpt spans at most 240 bytes beyond its match"
                                )]
                                let count = bytes.iter().filter(|byte| **byte == b'\n').count();
                                u64::try_from(count).map_err(|_width| lines)
                            };
                        let before = newlines(0..from)?.checked_add(1).ok_or(lines)?;
                        let span = newlines(from..to.saturating_sub(1))?;
                        let first = u64::from(*start_line);
                        if u64::from(*end_line).checked_sub(first) != Some(span)
                            || first < before
                            || (context.start == 0 && first != before)
                        {
                            return Err(lines);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

quanta_index_contract_base::native_serde_struct!(NativeTextPathRankingV1 {
    ranking: NativeTextResult,
    repositories: Vec<RepoId>
});
quanta_index_contract_base::native_serde_struct!(NativeTextPathResultV1 {
    ranked: NativeTextPathRankingV1,
    previews: Option<Vec<NativeTextPreview>>
});
quanta_index_contract_base::native_serde_struct!(NativeTextPathPageV1 {
    session: NativeReadSessionId,
    ranked: NativeTextPathRankingV1,
    remaining: u64,
    after: Option<NativeTextPathCursorV1>,
    code_search: Vec<Option<NativeCodeSearchMatch>>
});
quanta_index_contract_base::native_serde_struct!(NativeCodeSearchExcerpt {
    context: PreviewByteRange,
    text: String
});
quanta_index_contract_base::native_serde_enum!(NativeCodeSearchMatch {
    units {}
    newtypes {}
    records {
        Content = 0 {
            start: u64,
            end: u64,
            start_line: u32,
            end_line: u32,
            excerpt: Option<NativeCodeSearchExcerpt>
        },
        Path = 1 { start: u64, end: u64 }
    }
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FiniteScore;
    use crate::native_protocol::{
        NativeIndexRequest, NativeIndexResponse, NativeReadSessionRequest,
        compare_native_text_path_rank_v1,
    };

    #[test]
    fn ranked_repository_paths_order_score_then_repository_then_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo_a = RepoId::try_from("a")?;
        let repo_b = RepoId::try_from("b")?;
        let first = FiniteScore::new(2.0)?;
        let tied = FiniteScore::new(1.0)?;
        let mut rows = [
            (tied, &repo_b, "src/body.rs"),
            (tied, &repo_a, "src/zz.rs"),
            (tied, &repo_a, "src/body.rs"),
            (first, &repo_b, "src/zz.rs"),
        ];
        rows.sort_by(|left, right| compare_native_text_path_rank_v1(*left, *right));
        assert_eq!(
            rows,
            [
                (first, &repo_b, "src/zz.rs"),
                (tied, &repo_a, "src/body.rs"),
                (tied, &repo_a, "src/zz.rs"),
                (tied, &repo_b, "src/body.rs"),
            ]
        );
        Ok(())
    }

    #[test]
    fn retained_path_page_has_distinct_wire_and_preserves_the_repository_anchor()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = serde_json::json!({
            "workspace": {"tenant": vec![1_u8; 16], "collection": vec![2_u8; 16],
                "worktree": vec![3_u8; 16], "slot": vec![4_u8; 16]},
            "epoch": 1, "sequence": 9, "digest": vec![5_u8; 32]
        });
        let anchor = serde_json::json!({
            "view": view, "instance": vec![8_u8; 16], "query": vec![9_u8; 32],
            "artifact": vec![10_u8; 32], "score": 1.25, "repository": "corp-a", "path": "src/a.rs"
        });
        let request_fields = serde_json::json!({
            "session": vec![7_u8; 32], "instance": vec![8_u8; 16],
            "query": {"syntax": "native", "query_text": "select:path needle",
                "constraints": {"language_any_of": []}, "top_k": 1},
            "after": anchor
        });
        let request_wire = serde_json::json!({
            "ReadSession": {"SearchTextPathPageV1": request_fields}
        });
        let request: NativeIndexRequest = serde_json::from_value(request_wire.clone())?;
        assert!(matches!(
            &request,
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPathPageV1 {
                after: Some(cursor), ..
            }) if cursor.repository.as_str() == "corp-a" && cursor.path == "src/a.rs"
        ));
        assert_eq!(serde_json::to_value(&request)?, request_wire);
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&request, &mut bytes)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(&*bytes)?,
            request
        );
        let owner_page = serde_json::json!({"ReadSession": {"SearchTextPage": request_fields}});
        assert!(serde_json::from_value::<NativeIndexRequest>(owner_page).is_err());

        let reply_wire = serde_json::json!({"TextPathPageV1": {
            "session": vec![7_u8; 32], "remaining": 2, "after": anchor,
            "code_search": [{"Path": {"start": 4, "end": 5}}],
            "ranked": {
                "ranking": {
                    "view": view, "instance": vec![8_u8; 16], "coverage": "CoreRequired",
                    "artifact": vec![10_u8; 32], "matched": 2, "exhaustive": true,
                    "hits": [{"owner": {"producer": vec![11_u8; 16], "stable_id": vec![12_u8; 16]},
                        "source": vec![13_u8; 32], "path": "src/a.rs", "score": 1.25}]
                },
                "repositories": ["corp-a"]
            }
        }});
        let reply: NativeIndexResponse = serde_json::from_value(reply_wire.clone())?;
        let NativeIndexResponse::TextPathPageV1(page) = &reply else {
            return Err("path page gold decoded as another response".into());
        };
        struct Borrowed<'a>(&'a NativeTextPathPageV1);
        impl serde::Serialize for Borrowed<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_text_path_page_ref(self.0, serializer)
            }
        }
        let borrowed = Borrowed(page);
        assert_eq!(serde_json::to_value(&borrowed)?, reply_wire);
        bytes.clear();
        ciborium::ser::into_writer(&borrowed, &mut bytes)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(&*bytes)?,
            reply
        );
        Ok(())
    }

    fn code_search_page(
        path: &str,
        matched: Option<NativeCodeSearchMatch>,
    ) -> Result<NativeTextPathPageV1, Box<dyn std::error::Error>> {
        use crate::index::{
            CollectionId, Digest, IndexInstanceId, ObjectRef, OwnerKey, ProducerId, SlotId,
            StableOwnerId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
        };
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 9,
            digest: Digest([5; 32]),
        };
        Ok(NativeTextPathPageV1 {
            session: NativeReadSessionId([7; 32]),
            ranked: NativeTextPathRankingV1 {
                ranking: NativeTextResult {
                    view,
                    instance: IndexInstanceId([8; 16]),
                    coverage: super::super::NativeTextCoverage::CoreRequired,
                    artifact: ObjectRef(Digest([10; 32])),
                    matched: 1,
                    exhaustive: true,
                    hits: vec![super::super::NativeTextHit {
                        owner: OwnerKey {
                            producer: ProducerId([11; 16]),
                            stable_id: StableOwnerId([12; 16]),
                        },
                        source: ObjectRef(Digest([13; 32])),
                        path: path.into(),
                        score: FiniteScore::new(1.0)?,
                    }],
                },
                repositories: vec![RepoId::try_from("corp-a")?],
            },
            remaining: 1,
            after: None,
            code_search: vec![matched],
        })
    }

    fn content(
        start: u64,
        end: u64,
        lines: (u32, u32),
        context: (u64, u64),
        text: &str,
    ) -> NativeCodeSearchMatch {
        NativeCodeSearchMatch::Content {
            start,
            end,
            start_line: lines.0,
            end_line: lines.1,
            excerpt: Some(NativeCodeSearchExcerpt {
                context: PreviewByteRange {
                    start: context.0,
                    end: context.1,
                },
                text: text.into(),
            }),
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed wire gold and refusal assertions are independent test oracles"
    )]
    fn code_search_matches_round_trip_and_bind_to_their_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid = [
            Some(content(11, 15, (2, 2), (0, 21), "todo!()\nfn main() {}\n")),
            Some(NativeCodeSearchMatch::Content {
                start: 0,
                end: 4,
                start_line: 1,
                end_line: 1,
                excerpt: None,
            }),
            Some(NativeCodeSearchMatch::Path { start: 4, end: 9 }),
            None,
        ];
        for matched in valid {
            let page = code_search_page("src/alpha.rs", matched)?;
            page.validate_code_search()?;
            let json = serde_json::to_string(&page)?;
            assert_eq!(serde_json::from_str::<NativeTextPathPageV1>(&json)?, page);
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&page, &mut cbor)?;
            assert_eq!(
                ciborium::de::from_reader::<NativeTextPathPageV1, _>(cbor.as_slice())?,
                page
            );
        }
        let mut empty = code_search_page("src/alpha.rs", None)?;
        empty.code_search.clear();
        empty.validate_code_search()?;
        for refused in [
            // An empty range, zero or reversed lines.
            content(4, 4, (1, 1), (0, 8), "src/alph"),
            content(0, 4, (0, 1), (0, 4), "todo"),
            content(0, 4, (2, 1), (0, 4), "todo"),
            // A context that misses the match or whose text differs in width.
            content(5, 9, (1, 1), (6, 9), "ain"),
            content(0, 4, (1, 1), (0, 4), "tod"),
            // A context wider than 120 bytes on each side.
            content(200, 201, (1, 1), (0, 242), &"x".repeat(242)),
            NativeCodeSearchMatch::Path { start: 9, end: 4 },
            NativeCodeSearchMatch::Path { start: 4, end: 40 },
        ] {
            let page = code_search_page("src/alpha.rs", Some(refused.clone()))?;
            assert!(page.validate_code_search().is_err(), "{refused:?}");
        }
        // A content match must sit on character boundaries of its excerpt:
        // "t\u{e9}t" is 4 bytes, and 1..2 splits the two-byte `\u{e9}`.
        let split = code_search_page(
            "src/alpha.rs",
            Some(content(1, 2, (1, 1), (0, 4), "t\u{e9}t")),
        )?;
        assert!(split.validate_code_search().is_err());
        let whole = code_search_page(
            "src/alpha.rs",
            Some(content(1, 3, (1, 1), (0, 4), "t\u{e9}t")),
        )?;
        whole.validate_code_search()?;
        // A path match must sit on character boundaries of the row's path.
        let page = code_search_page(
            "src/\u{e9}.rs",
            Some(NativeCodeSearchMatch::Path { start: 4, end: 5 }),
        )?;
        assert!(page.validate_code_search().is_err());
        // One entry per row, or none at all.
        let mut extra = code_search_page("src/alpha.rs", None)?;
        extra.code_search.push(None);
        assert!(extra.validate_code_search().is_err());
        Ok(())
    }

    /// Every single-field mutation of a valid page has a hand-derived verdict.
    ///
    /// The verdicts follow the wire rules. A mutation that changes a match's lines,
    /// its excerpt's width, layout or alignment, or moves it off its row is
    /// refused. The accepted ones are other well-formed matches of the same
    /// row: the wire carries verified bytes, so only the daemon can tell them
    /// apart, and the table pins exactly which they are.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "hand-derived mutation verdicts are independent test oracles"
    )]
    fn every_code_search_field_mutation_has_a_hand_derived_verdict()
    -> Result<(), Box<dyn std::error::Error>> {
        // "todo!()\nfn main() {}\n": `main` is 11..15 on line 2.
        const TEXT: &str = "todo!()\nfn main() {}\n";
        let base = |start, end, lines, context: (u64, u64), text: &str| {
            Some(content(start, end, lines, context, text))
        };
        let path = |start, end| Some(NativeCodeSearchMatch::Path { start, end });
        let mut refused_count = 0_u32;
        for (name, mutated, accepted) in [
            (
                "start - 1 is ` main`",
                base(10, 15, (2, 2), (0, 21), TEXT),
                true,
            ),
            (
                "start + 1 is `ain`",
                base(12, 15, (2, 2), (0, 21), TEXT),
                true,
            ),
            (
                "start = end is empty",
                base(15, 15, (2, 2), (0, 21), TEXT),
                false,
            ),
            (
                "end - 1 is `mai`",
                base(11, 14, (2, 2), (0, 21), TEXT),
                true,
            ),
            (
                "end + 1 is `main(`",
                base(11, 16, (2, 2), (0, 21), TEXT),
                true,
            ),
            (
                "end past the context",
                base(11, 22, (2, 2), (0, 21), TEXT),
                false,
            ),
            (
                "start line 1 under a newline",
                base(11, 15, (1, 2), (0, 21), TEXT),
                false,
            ),
            (
                "start line after end line",
                base(11, 15, (3, 2), (0, 21), TEXT),
                false,
            ),
            (
                "end line 3 over no newline",
                base(11, 15, (2, 3), (0, 21), TEXT),
                false,
            ),
            (
                "end line before start line",
                base(11, 15, (2, 1), (0, 21), TEXT),
                false,
            ),
            (
                "both lines 3 from offset 0",
                base(11, 15, (3, 3), (0, 21), TEXT),
                false,
            ),
            (
                "context start + 1",
                base(11, 15, (2, 2), (1, 21), TEXT),
                false,
            ),
            (
                "context end - 1",
                base(11, 15, (2, 2), (0, 20), TEXT),
                false,
            ),
            // The same bytes claimed one byte later in the Source: line 2
            // is still possible, since lines before the context are unseen.
            (
                "context shifted by one",
                base(11, 15, (2, 2), (1, 22), TEXT),
                true,
            ),
            (
                "text drops a byte",
                base(11, 15, (2, 2), (0, 21), "todo!()\nfn main() {}"),
                false,
            ),
            (
                "text gains a byte",
                base(11, 15, (2, 2), (0, 21), "todo!()\nfn main() {}\n\n"),
                false,
            ),
            (
                "text same width, same layout",
                base(11, 15, (2, 2), (0, 21), "todo!()\nfn MAIN() {}\n"),
                true,
            ),
            (
                "text same width, newline moved",
                base(11, 15, (2, 2), (0, 21), "todo!() fn main()\n{}\n"),
                false,
            ),
            (
                "text splits a character",
                base(1, 2, (1, 1), (0, 4), "t\u{e9}t"),
                false,
            ),
            ("variant swapped to path", path(11, 15), false),
            ("path start - 1 is `/alph`", path(3, 9), true),
            ("path end past the path", path(4, 13), false),
            ("path start = end is empty", path(9, 9), false),
        ] {
            let page = code_search_page("src/alpha.rs", mutated)?;
            let verdict = page.validate_code_search();
            assert_eq!(verdict.is_ok(), accepted, "{name}: {verdict:?}");
            if !accepted {
                refused_count = refused_count.saturating_add(1);
            }
        }
        assert_eq!(refused_count, 16);
        Ok(())
    }
}
