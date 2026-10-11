//! Canonical bounded publication work stream shared by Kernel and Catalog.
use sha2::{Digest as _, Sha256};

use super::{Digest, IndexError, MapRoot, ObjectRef, WorkIntent, WorkScope};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkAction {
    Retire(WorkIntent),
    Create(WorkIntent),
}

impl WorkAction {
    /// Ordering is retire first, then create, and strict intent identity within
    /// each phase. It is independent of physical manifest page partition.
    pub fn key(&self) -> Result<Vec<u8>, IndexError> {
        let (tag, intent) = match self {
            Self::Retire(intent) => (0, intent),
            Self::Create(intent) => (1, intent),
        };
        let digest = intent
            .identity_digest()
            .map_err(|error| IndexError::InvalidInput(format!("work intent identity: {error}")))?;
        let mut key = Vec::with_capacity(33);
        key.push(tag);
        key.extend_from_slice(&digest.0);
        Ok(key)
    }

    /// Retirement compares historical stamps; only created work owns current
    /// immutable input dependencies.
    #[must_use]
    pub fn children(&self) -> Vec<ObjectRef> {
        let Self::Create(intent) = self else {
            return Vec::new();
        };
        match intent.scope {
            WorkScope::Owner { expected, .. } => {
                let mut refs: Vec<_> = [expected.source, expected.projection]
                    .into_iter()
                    .flatten()
                    .collect();
                refs.sort_unstable();
                refs.dedup();
                refs
            }
            WorkScope::ProjectionDomain { source, .. } => {
                let mut refs: Vec<_> = [source.owners.0, source.paths.0, source.frontiers.0]
                    .into_iter()
                    .flatten()
                    .collect();
                refs.sort_unstable();
                refs.dedup();
                refs
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkActionSet {
    pub root: MapRoot,
    pub count: u64,
    pub digest: Digest,
}

impl WorkActionSet {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            root: MapRoot(None),
            count: 0,
            digest: WorkActionDigest::default().finish(),
        }
    }
}

pub struct WorkActionDigest {
    hash: Sha256,
    count: u64,
    previous: Option<Vec<u8>>,
}

impl Default for WorkActionDigest {
    fn default() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.work-actions.v1\0");
        Self {
            hash,
            count: 0,
            previous: None,
        }
    }
}

impl WorkActionDigest {
    pub fn push(&mut self, action: &WorkAction) -> Result<(), IndexError> {
        let key = action.key()?;
        if self.previous.as_ref().is_some_and(|prior| prior >= &key) {
            return Err(IndexError::InvalidInput(
                "work actions must be sorted and unique".into(),
            ));
        }
        let count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.hash.update(&key);
        self.previous = Some(key);
        self.count = count;
        Ok(())
    }

    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn finish(&self) -> Digest {
        let mut hash = self.hash.clone();
        hash.update(self.count.to_be_bytes());
        Digest(hash.finalize().into())
    }
}

quanta_index_contract::native_serde_enum!(WorkAction { units { } newtypes { Retire = 0 => WorkIntent, Create = 1 => WorkIntent } records { } });
quanta_index_contract::native_serde_struct!(WorkActionSet {
    root: MapRoot,
    count: u64,
    digest: Digest
});
