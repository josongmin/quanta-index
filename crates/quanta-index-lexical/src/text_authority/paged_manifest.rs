//! Compact Native text-authority root committed by the outer lexical seal.
//!
//! Every map page and value is an authenticated immutable object. The root
//! contains no corpus-sized inventories; a normal open validates this control
//! row and selected reads validate the referenced leaves.

use quanta_index_core::CoreError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::lexical_store::{MapRoot, ObjectRef};
use crate::normalize::TEXT_NORMALIZER_VERSION;

pub(crate) const NATIVE_FORMAT: u16 = 6;

type NativeRootWire = (
    u16,
    (u16, u16),
    [u8; 32],
    MapRoot,
    MapRoot,
    MapRoot,
    MapRoot,
    MapRoot,
    MapRoot,
    MapRoot,
    u64,
    u64,
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeRoot {
    pub(crate) allocation_scope_sha256: [u8; 32],
    pub(crate) source: MapRoot,
    pub(crate) current: MapRoot,
    pub(crate) membership: MapRoot,
    pub(crate) segments: MapRoot,
    pub(crate) gram_routes: MapRoot,
    pub(crate) term_routes: MapRoot,
    /// Segments with retired locals awaiting bounded posting compaction.
    pub(crate) compaction_due: MapRoot,
    pub(crate) next_segment_id: u64,
    pub(crate) live_docs: u64,
}

impl NativeRoot {
    pub(crate) fn empty(allocation_scope_sha256: [u8; 32]) -> Self {
        Self {
            allocation_scope_sha256,
            source: MapRoot::default(),
            current: MapRoot::default(),
            membership: MapRoot::default(),
            segments: MapRoot::default(),
            gram_routes: MapRoot::default(),
            term_routes: MapRoot::default(),
            compaction_due: MapRoot::default(),
            next_segment_id: 1,
            live_docs: 0,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.allocation_scope_sha256 == [0; 32]
            || self.next_segment_id == 0
            || self.current.entries != self.live_docs
            || self.membership.entries != self.live_docs
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native root counters or map membership disagree".into(),
            ));
        }
        for map in [
            &self.source,
            &self.current,
            &self.membership,
            &self.segments,
            &self.gram_routes,
            &self.term_routes,
            &self.compaction_due,
        ] {
            if map.page.is_none() != (map.entries == 0) {
                return Err(CoreError::InvalidContract(
                    "lexical: Native root has inconsistent map page/count".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn root_objects(&self) -> Vec<ObjectRef> {
        [
            &self.source,
            &self.current,
            &self.membership,
            &self.segments,
            &self.gram_routes,
            &self.term_routes,
            &self.compaction_due,
        ]
        .into_iter()
        .filter_map(|map| map.page.as_ref().map(|page| page.object.clone()))
        .collect()
    }
}

impl Serialize for NativeRoot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            NATIVE_FORMAT,
            (TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor),
            self.allocation_scope_sha256,
            &self.source,
            &self.current,
            &self.membership,
            &self.segments,
            &self.gram_routes,
            &self.term_routes,
            &self.compaction_due,
            self.next_segment_id,
            self.live_docs,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NativeRoot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            format,
            normalizer,
            allocation_scope_sha256,
            source,
            current,
            membership,
            segments,
            gram_routes,
            term_routes,
            compaction_due,
            next_segment_id,
            live_docs,
        ): NativeRootWire = Deserialize::deserialize(deserializer)?;
        if format != NATIVE_FORMAT
            || normalizer != (TEXT_NORMALIZER_VERSION.major, TEXT_NORMALIZER_VERSION.minor)
        {
            return Err(serde::de::Error::custom(
                "unsupported Native text-authority format or normalizer",
            ));
        }
        let root = Self {
            allocation_scope_sha256,
            source,
            current,
            membership,
            segments,
            gram_routes,
            term_routes,
            compaction_due,
            next_segment_id,
            live_docs,
        };
        root.validate().map_err(serde::de::Error::custom)?;
        Ok(root)
    }
}
