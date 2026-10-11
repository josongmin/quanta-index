//! Fixed-width wire descriptors. Object and page payload codecs live beside
//! their readers; these tuple descriptors are shared by authority roots.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum ObjectKind {
    FileBody = 1,
    FilePostings = 2,
    NativeBody = 3,
    NativeTrigrams = 4,
    NativePositions = 5,
    Segment = 6,
    Record = 7,
    MapPage = 8,
    BlobBlock = 9,
    BlobNode = 10,
    AttemptBatch = 11,
}

impl ObjectKind {
    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::FileBody => 1,
            Self::FilePostings => 2,
            Self::NativeBody => 3,
            Self::NativeTrigrams => 4,
            Self::NativePositions => 5,
            Self::Segment => 6,
            Self::Record => 7,
            Self::MapPage => 8,
            Self::BlobBlock => 9,
            Self::BlobNode => 10,
            Self::AttemptBatch => 11,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::FileBody,
            2 => Self::FilePostings,
            3 => Self::NativeBody,
            4 => Self::NativeTrigrams,
            5 => Self::NativePositions,
            6 => Self::Segment,
            7 => Self::Record,
            8 => Self::MapPage,
            9 => Self::BlobBlock,
            10 => Self::BlobNode,
            11 => Self::AttemptBatch,
            _ => return None,
        })
    }

    pub(super) const fn is_payload(self) -> bool {
        !matches!(
            self,
            Self::MapPage | Self::BlobBlock | Self::BlobNode | Self::AttemptBatch
        )
    }
}

impl Serialize for ObjectKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.code())
    }
}

impl<'de> Deserialize<'de> for ObjectKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = u8::deserialize(deserializer)?;
        Self::from_code(code).ok_or_else(|| serde::de::Error::custom("unknown lexical object kind"))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ObjectRef {
    pub(crate) kind: ObjectKind,
    /// Encoded object bytes, including header, dependencies and payload.
    pub(crate) bytes: u64,
    pub(crate) sha256: [u8; 32],
}

impl Serialize for ObjectRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.kind, self.bytes, self.sha256).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ObjectRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (kind, bytes, sha256) = <(ObjectKind, u64, [u8; 32])>::deserialize(deserializer)?;
        Ok(Self {
            kind,
            bytes,
            sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct BlobRef {
    pub(crate) root: ObjectRef,
    /// Logical payload bytes, independent of physical block count.
    pub(crate) bytes: u64,
    pub(crate) sha256: [u8; 32],
}

impl Serialize for BlobRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.root, self.bytes, self.sha256).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (root, bytes, sha256) = <(ObjectRef, u64, [u8; 32])>::deserialize(deserializer)?;
        Ok(Self {
            root,
            bytes,
            sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PageRef {
    pub(crate) object: ObjectRef,
    pub(crate) first: Vec<u8>,
    pub(crate) last: Vec<u8>,
    pub(crate) entries: u64,
    pub(crate) height: u8,
}

impl Serialize for PageRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.object,
            &self.first,
            &self.last,
            self.entries,
            self.height,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PageRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (object, first, last, entries, height) =
            <(ObjectRef, Vec<u8>, Vec<u8>, u64, u8)>::deserialize(deserializer)?;
        Ok(Self {
            object,
            first,
            last,
            entries,
            height,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MapRoot {
    pub(crate) page: Option<PageRef>,
    pub(crate) entries: u64,
}

impl Serialize for MapRoot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.page, self.entries).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MapRoot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (page, entries) = <(Option<PageRef>, u64)>::deserialize(deserializer)?;
        Ok(Self { page, entries })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MapEdit {
    pub(crate) key: Vec<u8>,
    pub(crate) value: Option<BlobRef>,
}
