//! Per-track readiness values: the in-memory `TrackLedger`, the persisted
//! `TrackAuthorityState`, and the per-generation `SemanticGenerationState`.

use std::fmt;

use quanta_index_contract::ManifestGeneration;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::serde_support::impl_struct_serde;

/// Per-track readiness state.
#[derive(Debug, Default)]
pub struct TrackLedger {
    materialized: Option<ManifestGeneration>,
    sealed: Option<ManifestGeneration>,
    manifest_digest: Option<String>,
}

impl TrackLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn materialized(&self) -> Option<ManifestGeneration> {
        self.materialized
    }

    #[must_use]
    pub fn sealed(&self) -> Option<ManifestGeneration> {
        self.sealed
    }

    #[must_use]
    pub fn manifest_digest(&self) -> Option<&str> {
        self.manifest_digest.as_deref()
    }

    /// Monotonic materialization update. Lower generations do not rewind
    /// readiness truth.
    pub fn record_materialized(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        let next = match self.materialized {
            Some(current) if current.get() > generation.get() => current,
            _ => generation,
        };
        self.materialized = Some(next);
        if self
            .materialized
            .is_some_and(|current| current.get() == generation.get())
            && let Some(digest) = manifest_digest
        {
            self.manifest_digest = Some(digest.to_string());
        }
    }

    /// Monotonic seal update. Lower generations do not rewind readiness.
    pub fn record_seal(&mut self, generation: ManifestGeneration, manifest_digest: Option<&str>) {
        self.record_materialized(generation, manifest_digest);
        let next = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(next);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrackAuthorityState {
    pub(super) materialized: Option<ManifestGeneration>,
    pub(super) sealed: Option<ManifestGeneration>,
    pub(super) manifest_digest: Option<String>,
}

impl TrackAuthorityState {
    pub(crate) fn record_materialized(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        let next = match self.materialized {
            Some(current) if current.get() > generation.get() => current,
            _ => generation,
        };
        self.materialized = Some(next);
        if self
            .materialized
            .is_some_and(|current| current.get() == generation.get())
            && let Some(digest) = manifest_digest
        {
            self.manifest_digest = Some(digest.to_string());
        }
    }

    pub(crate) fn record_seal(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        self.record_materialized(generation, manifest_digest);
        let next = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(next);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SemanticGenerationState {
    manifest_digest: String,
    materialized: bool,
    sealed: bool,
}

impl SemanticGenerationState {
    #[must_use]
    pub(crate) fn manifest_digest(&self) -> &str {
        self.manifest_digest.as_str()
    }

    #[must_use]
    pub(crate) const fn materialized(&self) -> bool {
        self.materialized
    }

    #[must_use]
    pub(crate) const fn sealed(&self) -> bool {
        self.sealed
    }

    pub(super) fn record_materialized(&mut self, manifest_digest: &str) {
        self.materialized = true;
        self.manifest_digest = manifest_digest.to_string();
    }

    pub(super) fn record_sealed(&mut self, manifest_digest: &str) {
        self.record_materialized(manifest_digest);
        self.sealed = true;
    }
}

impl_struct_serde!(TrackAuthorityState {
    materialized: Option<ManifestGeneration>,
    sealed: Option<ManifestGeneration>,
    manifest_digest: Option<String>,
});

impl_struct_serde!(SemanticGenerationState {
    manifest_digest: String,
    materialized: bool,
    sealed: bool,
});
