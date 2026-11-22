//! Native `select:path` returns one ranked representative per repository path.

use super::read_session::{NativeReadSessionId, NativeTextPathCursorV1};
use super::{NativeTextPreview, NativeTextResult};
use crate::RepoId;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPathPageV1 {
    pub session: NativeReadSessionId,
    pub ranked: NativeTextPathRankingV1,
    /// Groups strictly after the input cursor, before the current page cap.
    pub remaining: u64,
    pub after: Option<NativeTextPathCursorV1>,
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
    after: Option<NativeTextPathCursorV1>
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
}
