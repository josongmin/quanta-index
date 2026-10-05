//! Transport staging for one unchanged, sealed source publication.
//!
//! Staging is not a source event reservation or a generation mutation. The
//! original batch is dispatched only after its complete body is verified.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::corpus_wire::{SourceBytesBuf, SourceBytesWire};

pub const SOURCE_PUBLICATION_UPLOAD_PART_BYTES: usize = 1024 * 1024;
pub const SOURCE_PUBLICATION_UPLOAD_MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePublicationUploadIdentity {
    pub body_sha256: [u8; 32],
    pub body_bytes: u64,
}

impl Serialize for SourcePublicationUploadIdentity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.body_sha256, self.body_bytes).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourcePublicationUploadIdentity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (body_sha256, body_bytes) = Deserialize::deserialize(deserializer)?;
        Ok(Self { body_sha256, body_bytes })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePublicationUploadPart {
    pub identity: SourcePublicationUploadIdentity,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

impl Serialize for SourcePublicationUploadPart {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.identity, self.offset, SourceBytesWire(&self.bytes)).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourcePublicationUploadPart {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (identity, offset, bytes): (_, _, SourceBytesBuf) = Deserialize::deserialize(deserializer)?;
        Ok(Self { identity, offset, bytes: bytes.0 })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePublicationUploadAck {
    pub identity: SourcePublicationUploadIdentity,
    pub next_offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePublicationUploadCommit {
    pub identity: SourcePublicationUploadIdentity,
    pub publication: crate::SourcePublicationBinding,
}

impl Serialize for SourcePublicationUploadCommit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.identity, &self.publication).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourcePublicationUploadCommit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (identity, publication) = Deserialize::deserialize(deserializer)?;
        Ok(Self { identity, publication })
    }
}

impl Serialize for SourcePublicationUploadAck {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.identity, self.next_offset).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourcePublicationUploadAck {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (identity, next_offset) = Deserialize::deserialize(deserializer)?;
        Ok(Self { identity, next_offset })
    }
}
