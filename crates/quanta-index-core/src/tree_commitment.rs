//! File commitments and mismatch outcomes shared by tree readers and sealers.

use std::fmt;

/// One file a sealed generation commits to: its `/`-joined path relative to
/// the generation directory, its length and its SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedArtifactCommitmentV1 {
    pub name: String,
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// The first way a tree differed from its commitment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeCommitmentMismatchV1 {
    /// A committed file is gone.
    Missing { name: String },
    /// A file exists that the seal did not commit to; for a versioned
    /// dataset an extra file can change what opens, so it is a defect.
    Extra { name: String },
    /// A committed file has a different length.
    Length {
        name: String,
        on_disk: u64,
        committed: u64,
    },
    /// A committed file has different content.
    Digest { name: String },
}

impl fmt::Display for TreeCommitmentMismatchV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { name } => write!(formatter, "{name}: missing"),
            Self::Extra { name } => {
                write!(
                    formatter,
                    "{name}: present although the seal did not commit to it"
                )
            }
            Self::Length {
                name,
                on_disk,
                committed,
            } => write!(
                formatter,
                "{name}: {on_disk} bytes on disk, {committed} committed"
            ),
            Self::Digest { name } => {
                write!(
                    formatter,
                    "{name}: content digest differs from the committed digest"
                )
            }
        }
    }
}

impl std::error::Error for TreeCommitmentMismatchV1 {}
