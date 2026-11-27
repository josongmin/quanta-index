//! Text matches on a Native connection: every match of one literal or regex,
//! one page at a time, under the contract of `NativeTextMatchesRequest`.
//!
//! A first page reads the newest view of the connection's Native selection.
//! Its continuation token carries the Native match cursor, which binds that
//! view, the text member and the query, and every later page reads the same
//! view even after later publications. Sessions follow the ownership model of
//! the default `.text()` route: one held lease per view, renewed in the
//! background, released when no cursor on it is outstanding or when the
//! client is dropped; see `NativeViewSessions`. Each token carries an SDK
//! cursor identity, so the lease of its view stays held while that cursor
//! is live.

use quanta_index_contract::index::{ViewRef, ViewSelection};
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_TEXT_MATCH_LIMIT, NativeTextMatch, NativeTextMatchCursor, NativeTextMatchQuery,
    NativeTextMatchesRequest,
};
use quanta_index_contract::{ContinuationTokenV2, SearchPlaneErrorCodeV2};
use serde::ser::SerializeStruct as _;

use crate::SdkError;
use crate::native_code_search::{
    NativeTextRoute, decode_token, encode_token, native_transport, open_key, selection_contains,
};

/// Continuation tokens minted by this route start with this tag.
const CURSOR_TAG: &str = "native-text-matches:";

/// The match cursor and the SDK identity of the token that carries it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TextMatchContinuation {
    cursor_id: [u8; 32],
    native: NativeTextMatchCursor,
}

impl serde::Serialize for TextMatchContinuation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("TextMatchContinuation", 2)?;
        fields.serialize_field("cursor_id", &self.cursor_id)?;
        fields.serialize_field("native", &self.native)?;
        fields.end()
    }
}

impl<'de> serde::Deserialize<'de> for TextMatchContinuation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["cursor_id", "native"];

        enum Field {
            CursorId,
            Native,
            Ignored,
        }

        impl<'de> serde::Deserialize<'de> for Field {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct FieldVisitor;
                impl<'de> serde::de::Visitor<'de> for FieldVisitor {
                    type Value = Field;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("field identifier")
                    }

                    fn visit_str<E: serde::de::Error>(self, field: &str) -> Result<Field, E> {
                        self.visit_bytes(field.as_bytes())
                    }

                    fn visit_bytes<E: serde::de::Error>(self, field: &[u8]) -> Result<Field, E> {
                        match field {
                            b"cursor_id" => Ok(Field::CursorId),
                            b"native" => Ok(Field::Native),
                            _ => Ok(Field::Ignored),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct ContinuationVisitor;
        impl<'de> serde::de::Visitor<'de> for ContinuationVisitor {
            type Value = TextMatchContinuation;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct TextMatchContinuation")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut cursor_id = None;
                let mut native = None;
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::CursorId if cursor_id.is_none() => {
                            cursor_id = Some(map.next_value()?);
                        }
                        Field::Native if native.is_none() => {
                            native = Some(map.next_value()?);
                        }
                        Field::CursorId => {
                            return Err(serde::de::Error::duplicate_field("cursor_id"));
                        }
                        Field::Native => {
                            return Err(serde::de::Error::duplicate_field("native"));
                        }
                        // Preserve the derived envelope's decode semantics.
                        // The shared token codec rejects the noncanonical map.
                        Field::Ignored => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(TextMatchContinuation {
                    cursor_id: cursor_id
                        .ok_or_else(|| serde::de::Error::missing_field("cursor_id"))?,
                    native: native.ok_or_else(|| serde::de::Error::missing_field("native"))?,
                })
            }

            fn visit_seq<S: serde::de::SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> Result<Self::Value, S::Error> {
                let cursor_id = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let native = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(3, &self));
                }
                Ok(TextMatchContinuation { cursor_id, native })
            }
        }

        deserializer.deserialize_struct("TextMatchContinuation", FIELDS, ContinuationVisitor)
    }
}

/// One page of text matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextMatchesResponse {
    /// The view every page of this sequence reads.
    pub view: ViewRef,
    /// Rows ordered by repository (unscoped first), path and start byte.
    pub matches: Vec<NativeTextMatch>,
    /// Present exactly when more matches follow this page.
    pub next_cursor: Option<ContinuationTokenV2>,
}

impl serde::Serialize for NativeTextMatchesResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("NativeTextMatchesResponse", 3)?;
        fields.serialize_field("view", &self.view)?;
        fields.serialize_field("matches", &self.matches)?;
        fields.serialize_field("next_cursor", &self.next_cursor)?;
        fields.end()
    }
}

/// Read one page of `query` matches, `limit` rows at most, after `after`.
pub(crate) fn execute(
    route: &NativeTextRoute<'_>,
    query: NativeTextMatchQuery,
    limit: u32,
    after: Option<&ContinuationTokenV2>,
) -> Result<NativeTextMatchesResponse, SdkError> {
    if !(1..=MAX_NATIVE_TEXT_MATCH_LIMIT).contains(&limit) {
        return Err(SdkError::Usage("text match limit must be 1..=1024".into()));
    }
    query
        .validate()
        .map_err(|message| SdkError::Usage(message.into()))?;
    let after = after.map(decode_cursor).transpose()?;
    let consumed = after.as_ref().map(|cursor| cursor.cursor_id);
    let after = after.map(|cursor| cursor.native);
    let budget = route.budget();
    let view = match &after {
        Some(cursor) => cursor.view,
        None => {
            route
                .client
                .read_view(route.selection, &budget)
                .map_err(native_transport)?
                .identity
        }
    };
    if !selection_contains(route.selection, view) {
        return Err(SdkError::Usage(
            "the Native view lies outside this connection's Native selection".into(),
        ));
    }
    let page = route.sessions.on_view(
        route.client,
        view,
        route.text_instance,
        consumed,
        &budget,
        None,
        |client, status, spec, _observation| {
            if after
                .as_ref()
                .is_some_and(|cursor| cursor.instance != spec.instance)
            {
                return Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CursorInvalid,
                    message: "the continuation belongs to another Native text member".into(),
                    repair: None,
                });
            }
            let request = NativeTextMatchesRequest {
                selection: ViewSelection::Exact(view),
                instance: spec.instance,
                query,
                after,
                limit,
            };
            if let Err(message) = request.validate() {
                return Err(SdkError::Usage(message));
            }
            let page = client
                .search_text_matches_in_read_session(status, &request, &budget)
                .map_err(native_transport)?;
            let issued = page
                .next
                .as_ref()
                .map(|next| {
                    let cursor_id = open_key(next.view)?;
                    Ok::<_, SdkError>((encode_cursor(next, cursor_id)?, cursor_id))
                })
                .transpose()?;
            let id = issued.as_ref().map(|(_, cursor_id)| *cursor_id);
            Ok(((page, issued.map(|(token, _)| token)), id))
        },
    )?;
    let (page, next_cursor) = page;
    Ok(NativeTextMatchesResponse {
        view: page.view,
        next_cursor,
        matches: page.matches,
    })
}

fn encode_cursor(
    cursor: &NativeTextMatchCursor,
    cursor_id: [u8; 32],
) -> Result<ContinuationTokenV2, SdkError> {
    encode_token(
        CURSOR_TAG,
        &TextMatchContinuation {
            cursor_id,
            native: cursor.clone(),
        },
    )
}

fn decode_cursor(token: &ContinuationTokenV2) -> Result<TextMatchContinuation, SdkError> {
    decode_token(CURSOR_TAG, "the Native text-match route", token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::value::Value;
    use quanta_index_contract::index::{
        CollectionId, Digest, IndexInstanceId, ObjectRef, OwnerKey, ProducerId, SlotId,
        StableOwnerId, TenantId, WorkspaceKey, WorktreeId,
    };
    use quanta_index_contract::{NativeSourceIdentity, RepoId};

    #[test]
    fn match_cursor_tokens_round_trip_and_refuse_other_routes() {
        let cursor = NativeTextMatchCursor {
            view: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                sequence: 7,
                digest: Digest([5; 32]),
            },
            instance: IndexInstanceId([6; 16]),
            query: Digest([7; 32]),
            repository: Some(RepoId::new("repo-e2e").expect("valid repository")),
            path: "src/a.rs".into(),
            native_source: NativeSourceIdentity {
                owner: OwnerKey {
                    producer: ProducerId([8; 16]),
                    stable_id: StableOwnerId([9; 16]),
                },
                source: ObjectRef(Digest([10; 32])),
            },
            start: 13,
        };
        let token = encode_cursor(&cursor, [11; 32]).expect("cursor encodes");
        assert!(token.as_str().starts_with(CURSOR_TAG));
        assert_eq!(
            decode_cursor(&token).expect("cursor decodes"),
            TextMatchContinuation {
                cursor_id: [11; 32],
                native: cursor.clone(),
            }
        );
        // Keep the derived struct's exact top-level CBOR key order and values.
        let expected = Value::Map(vec![
            (
                Value::Text("cursor_id".into()),
                Value::serialized(&[11_u8; 32]).expect("cursor id value"),
            ),
            (
                Value::Text("native".into()),
                Value::serialized(&cursor).expect("native cursor value"),
            ),
        ]);
        assert_eq!(
            token,
            encode_token(CURSOR_TAG, &expected).expect("fixed cursor envelope")
        );
        let Value::Map(mut fields) = expected else {
            panic!("fixture is a keyed cursor envelope");
        };
        fields.push((Value::Text("extra".into()), Value::Null));
        let extended = encode_token(CURSOR_TAG, &Value::Map(fields)).expect("extended envelope");
        assert!(matches!(
            decode_cursor(&extended),
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::CursorInvalid,
                message,
                ..
            }) if message == "Native cursor: not the canonical token of its cursor"
        ));
        // A code-search token is never a match cursor, and vice versa.
        let foreign = ContinuationTokenV2::new(format!(
            "native-code-search-path:{}",
            token.as_str().trim_start_matches(CURSOR_TAG)
        ))
        .expect("token");
        assert!(matches!(
            decode_cursor(&foreign),
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::CursorInvalid,
                ..
            })
        ));
    }

    #[test]
    fn match_response_keeps_public_field_order() {
        let response = NativeTextMatchesResponse {
            view: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                sequence: 7,
                digest: Digest([5; 32]),
            },
            matches: Vec::new(),
            next_cursor: None,
        };
        let mut actual = Vec::new();
        ciborium::ser::into_writer(&response, &mut actual).expect("response CBOR");
        let mut expected = Vec::new();
        ciborium::ser::into_writer(
            &Value::Map(vec![
                (
                    Value::Text("view".into()),
                    Value::serialized(&response.view).expect("view value"),
                ),
                (Value::Text("matches".into()), Value::Array(Vec::new())),
                (Value::Text("next_cursor".into()), Value::Null),
            ]),
            &mut expected,
        )
        .expect("fixed response envelope");
        assert_eq!(actual, expected);
    }
}
