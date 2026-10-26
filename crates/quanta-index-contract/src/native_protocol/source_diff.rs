//! Exact source-owner changes since the selected projection's certified cut.

use super::{
    OwnerKey, OwnerMutation, ProducerId, ProjectionBinding, ProjectionId, SourceCut,
    SourceOwnerItem, StableOwnerId, ViewRef, deserialize_source_owner_items,
    validate_source_owner_items,
};

impl SourceOwnerItem for OwnerMutation {
    fn owner(&self) -> OwnerKey {
        self.owner
    }
}

/// Zero to 128 ordered, non-noop source mutations. Reuses the canonical
/// expected/next contract, including actual deletions with next=None.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceDiffRows(Vec<OwnerMutation>);

impl NativeSourceDiffRows {
    pub fn new(rows: Vec<OwnerMutation>) -> Result<Self, &'static str> {
        validate_source_owner_items::<_, true>(&rows)?;
        if rows.iter().any(|row| row.expected == row.next) {
            return Err("source diff contains an unchanged owner");
        }
        Ok(Self(rows))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[OwnerMutation] {
        &self.0
    }
}

impl serde::Serialize for NativeSourceDiffRows {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for NativeSourceDiffRows {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rows = deserialize_source_owner_items::<_, _, true>(deserializer)?;
        Self::new(rows).map_err(serde::de::Error::custom)
    }
}

/// A page is bound to one exact view and its selected projection binding.
///
/// An absent binding explicitly means first materialization from
/// the empty source cut. Continuations must use `Exact(view)`, never `Latest`.
///
/// This proves Source changes only; parser policy or dependency changes need
/// their own affected-domain decision before reusing projection outputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProjectionSourceDiff {
    pub view: ViewRef,
    pub projection: ProjectionId,
    pub binding: Option<ProjectionBinding>,
    pub source: SourceCut,
    pub producer: ProducerId,
    pub rows: NativeSourceDiffRows,
    pub has_more: bool,
}

impl NativeProjectionSourceDiff {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.has_more && self.rows.as_slice().is_empty() {
            return Err("source diff continuation requires a nonempty page");
        }
        if self
            .binding
            .is_some_and(|binding| binding.projection != self.projection)
        {
            return Err("source diff projection binding differs");
        }
        if self
            .rows
            .as_slice()
            .iter()
            .any(|row| row.owner.producer != self.producer)
        {
            return Err("source diff row belongs to another producer");
        }
        Ok(())
    }

    #[must_use]
    pub fn next_after(&self) -> Option<StableOwnerId> {
        if self.has_more {
            self.rows.as_slice().last().map(|row| row.owner.stable_id)
        } else {
            None
        }
    }
}

impl serde::Serialize for NativeProjectionSourceDiff {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut record = serializer.serialize_struct("NativeProjectionSourceDiff", 7)?;
        record.serialize_field("view", &self.view)?;
        record.serialize_field("projection", &self.projection)?;
        record.serialize_field("binding", &self.binding)?;
        record.serialize_field("source", &self.source)?;
        record.serialize_field("producer", &self.producer)?;
        record.serialize_field("rows", &self.rows)?;
        record.serialize_field("has_more", &self.has_more)?;
        record.end()
    }
}

impl<'de> serde::Deserialize<'de> for NativeProjectionSourceDiff {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{Error as _, MapAccess, Visitor};
        const FIELDS: &[&str] = &[
            "view",
            "projection",
            "binding",
            "source",
            "producer",
            "rows",
            "has_more",
        ];
        struct PageVisitor;
        impl<'de> Visitor<'de> for PageVisitor {
            type Value = NativeProjectionSourceDiff;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a complete closed Native projection source diff")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let (
                    mut view,
                    mut projection,
                    mut binding,
                    mut source,
                    mut producer,
                    mut rows,
                    mut has_more,
                ) = (None, None, None, None, None, None, None);
                macro_rules! read {
                    ($slot:ident) => {{
                        if $slot.is_some() {
                            return Err(M::Error::duplicate_field(stringify!($slot)));
                        }
                        $slot = Some(map.next_value()?);
                    }};
                }
                while let Some(field) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match field {
                        "view" => read!(view),
                        "projection" => read!(projection),
                        "binding" => read!(binding),
                        "source" => read!(source),
                        "producer" => read!(producer),
                        "rows" => read!(rows),
                        "has_more" => read!(has_more),
                        _ => return Err(M::Error::unknown_field(field, FIELDS)),
                    }
                }
                let page = NativeProjectionSourceDiff {
                    view: view.ok_or_else(|| M::Error::missing_field("view"))?,
                    projection: projection.ok_or_else(|| M::Error::missing_field("projection"))?,
                    binding: binding.ok_or_else(|| M::Error::missing_field("binding"))?,
                    source: source.ok_or_else(|| M::Error::missing_field("source"))?,
                    producer: producer.ok_or_else(|| M::Error::missing_field("producer"))?,
                    rows: rows.ok_or_else(|| M::Error::missing_field("rows"))?,
                    has_more: has_more.ok_or_else(|| M::Error::missing_field("has_more"))?,
                };
                page.validate().map_err(M::Error::custom)?;
                Ok(page)
            }
        }
        deserializer.deserialize_struct("NativeProjectionSourceDiff", FIELDS, PageVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        index::{
            CollectionId, Digest, MapRoot, ObjectRef, SlotId, TenantId, WorkspaceKey, WorktreeId,
        },
        native_protocol::{NativeIndexRequest, NativeIndexResponse},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn require(value: bool, message: &'static str) -> TestResult {
        if value { Ok(()) } else { Err(message.into()) }
    }
    fn page() -> Result<NativeProjectionSourceDiff, &'static str> {
        let producer = ProducerId([5; 16]);
        Ok(NativeProjectionSourceDiff {
            view: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                sequence: 2,
                digest: Digest([6; 32]),
            },
            projection: ProjectionId([7; 16]),
            binding: None,
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            producer,
            rows: NativeSourceDiffRows::new(vec![
                OwnerMutation {
                    owner: OwnerKey {
                        producer,
                        stable_id: StableOwnerId([1; 16]),
                    },
                    expected: Some(ObjectRef(Digest([8; 32]))),
                    next: None,
                },
                OwnerMutation {
                    owner: OwnerKey {
                        producer,
                        stable_id: StableOwnerId([2; 16]),
                    },
                    expected: None,
                    next: Some(ObjectRef(Digest([9; 32]))),
                },
            ])?,
            has_more: false,
        })
    }

    struct Borrowed<'a>(&'a NativeProjectionSourceDiff);
    impl serde::Serialize for Borrowed<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_projection_source_diff_ref(self.0, serializer)
        }
    }

    #[test]
    fn source_diff_canonical_json_cbor_and_borrowed_emission_are_identical() -> TestResult {
        let page = page()?;
        let request = NativeIndexRequest::DiffProjectionSourceOwners {
            selection: crate::index::ViewSelection::Exact(page.view),
            projection: page.projection,
            producer: page.producer,
            after: None,
            limit: 2,
        };
        let json = serde_json::to_vec(&request)?;
        require(
            serde_json::from_slice::<NativeIndexRequest>(&json)? == request,
            "request roundtrip differs",
        )?;
        let mut request_cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut request_cbor)?;
        require(
            ciborium::de::from_reader::<NativeIndexRequest, _>(request_cbor.as_slice())? == request,
            "CBOR request differs",
        )?;
        let response = NativeIndexResponse::ProjectionSourceDiff(page.clone());
        let json = serde_json::to_vec(&response)?;
        require(
            serde_json::from_slice::<NativeIndexResponse>(&json)? == response,
            "response roundtrip differs",
        )?;
        require(
            json == serde_json::to_vec(&Borrowed(&page))?,
            "borrowed JSON differs",
        )?;
        let mut canonical = Vec::new();
        let mut borrowed = Vec::new();
        ciborium::ser::into_writer(&response, &mut canonical)?;
        ciborium::ser::into_writer(&Borrowed(&page), &mut borrowed)?;
        require(canonical == borrowed, "borrowed CBOR differs")?;
        require(
            ciborium::de::from_reader::<NativeIndexResponse, _>(canonical.as_slice())? == response,
            "CBOR response differs",
        )?;
        require(page.next_after().is_none(), "final page returned a cursor")?;
        Ok(())
    }

    #[test]
    fn source_diff_rejects_noop_duplicate_unordered_and_oversized_rows() -> TestResult {
        let page = page()?;
        let rows = page.rows.as_slice();
        let first = rows.first().ok_or("missing first row")?.clone();
        let last = rows.last().ok_or("missing last row")?.clone();
        for invalid in [
            vec![first.clone(), first.clone()],
            vec![last, first.clone()],
            vec![OwnerMutation {
                next: first.expected,
                ..first.clone()
            }],
            vec![OwnerMutation {
                expected: None,
                next: None,
                ..first
            }],
        ] {
            require(
                NativeSourceDiffRows::new(invalid.clone()).is_err(),
                "invalid rows constructed",
            )?;
            require(
                serde_json::from_slice::<NativeSourceDiffRows>(&serde_json::to_vec(&invalid)?)
                    .is_err(),
                "invalid rows decoded",
            )?;
        }
        let oversized = (0_u8..129)
            .map(|id| OwnerMutation {
                owner: OwnerKey {
                    producer: page.producer,
                    stable_id: StableOwnerId([id; 16]),
                },
                expected: None,
                next: Some(ObjectRef(Digest([9; 32]))),
            })
            .collect::<Vec<_>>();
        require(
            NativeSourceDiffRows::new(oversized.clone()).is_err(),
            "oversized rows constructed",
        )?;
        require(
            serde_json::from_slice::<NativeSourceDiffRows>(&serde_json::to_vec(&oversized)?)
                .is_err(),
            "oversized JSON decoded",
        )?;
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&oversized, &mut cbor)?;
        require(
            ciborium::de::from_reader::<NativeSourceDiffRows, _>(cbor.as_slice()).is_err(),
            "oversized CBOR decoded",
        )?;
        Ok(())
    }

    #[test]
    fn source_diff_closed_page_rejects_missing_duplicate_unknown_and_foreign_fields() -> TestResult
    {
        let page = page()?;
        let valid = serde_json::to_value(&page)?;
        for field in [
            "view",
            "projection",
            "binding",
            "source",
            "producer",
            "rows",
            "has_more",
        ] {
            let mut invalid = valid.clone();
            let _removed = invalid
                .as_object_mut()
                .ok_or("page is not an object")?
                .remove(field);
            require(
                serde_json::from_value::<NativeProjectionSourceDiff>(invalid).is_err(),
                "missing field decoded",
            )?;
        }
        let mut unknown = valid;
        let _previous = unknown
            .as_object_mut()
            .ok_or("page is not an object")?
            .insert("unknown".into(), serde_json::Value::Null);
        require(
            serde_json::from_value::<NativeProjectionSourceDiff>(unknown).is_err(),
            "unknown field decoded",
        )?;
        let duplicate = serde_json::to_string(&page)?.replace(
            "\"has_more\":false",
            "\"has_more\":false,\"has_more\":false",
        );
        require(
            serde_json::from_str::<NativeProjectionSourceDiff>(&duplicate).is_err(),
            "duplicate field decoded",
        )?;
        let mut foreign = page.clone();
        foreign.producer = ProducerId([0xff; 16]);
        require(
            serde_json::to_vec(&foreign).is_err(),
            "foreign producer serialized",
        )?;
        let mut empty = page;
        empty.rows = NativeSourceDiffRows::new(vec![])?;
        empty.has_more = true;
        require(
            serde_json::to_vec(&empty).is_err(),
            "empty continuation serialized",
        )?;
        empty.has_more = false;
        require(
            serde_json::from_slice::<NativeProjectionSourceDiff>(&serde_json::to_vec(&empty)?)?
                == empty,
            "empty final page differs",
        )?;
        Ok(())
    }
}
