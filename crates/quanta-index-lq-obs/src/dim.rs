//! Closed metric / span dimension set per OBS-01 § 4.3.
//!
//! Every metric sample and every span event carries the [`Dimensions`]
//! struct verbatim. The label set is closed by construction — adding a
//! new field requires an `lq_version` minor bump (RFC § Migration and
//! Versioning Policy) plus a coordinated edit to `Dimensions`, the
//! serde impl below, and the per-dimension cardinality guard.
//!
//! D18 — hand-rolled serde, no proc-macro derives.

use core::fmt;

use crate::errors::{ObsError, ObsErrorCode};

/// Per-field character cap — protects log/metric backends against
/// unbounded-length labels from misconfigured tenants.
pub const PER_FIELD_CHAR_CAP: usize = 256;

/// Layer-B global tenant cap per OBS-01 § 4.4.
///
/// Sized for the [`feature-scope.md`
/// § 7](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/feature-scope.md)
/// 100,000-repo capacity bound × ≈ 1× per-tenant safety margin.
pub const MAX_DISTINCT_TENANTS_GLOBAL: u32 = 100_000;

/// Layer-B per-tenant repo cap per OBS-01 § 4.4. Beyond this the metric backend
/// would explode on a single tenant's repos.
pub const MAX_DISTINCT_REPOS_PER_TENANT: u32 = 10_000;

/// Closed-label dimension set carried by every metric and every span event.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[expect(
    clippy::struct_field_names,
    reason = "field names ticket_id/wave_id/tenant_id/repo_id/generation_id are spec-locked by OBS-01 § 4.3"
)]
pub struct Dimensions {
    /// Ticket identifier (`OBS-01`, `LEX-05`, …). Cardinality bounded by the
    /// ticket pack.
    pub ticket_id: Box<str>,
    /// Wave identifier (`0`–`8`). Cardinality bounded.
    pub wave_id: Box<str>,
    /// Opaque tenant id; per-field cap [`PER_FIELD_CHAR_CAP`].
    pub tenant_id: Box<str>,
    /// Repo id within the tenant; per-field cap [`PER_FIELD_CHAR_CAP`].
    pub repo_id: Box<str>,
    /// Manifest generation; strictly monotonic per `(tenant, repo)`.
    pub generation_id: u64,
}

impl Dimensions {
    /// Build a [`Dimensions`] without validation. Use [`validate_dimensions`]
    /// at the emit boundary; never panic on construction.
    #[must_use]
    pub fn new(
        ticket_id: impl Into<Box<str>>,
        wave_id: impl Into<Box<str>>,
        tenant_id: impl Into<Box<str>>,
        repo_id: impl Into<Box<str>>,
        generation_id: u64,
    ) -> Self {
        Self {
            ticket_id: ticket_id.into(),
            wave_id: wave_id.into(),
            tenant_id: tenant_id.into(),
            repo_id: repo_id.into(),
            generation_id,
        }
    }
}

/// Structural validation of a [`Dimensions`] payload.
///
/// Fails closed when:
///
/// - any of `ticket_id`, `wave_id`, `tenant_id`, `repo_id` is empty —
///   surfaces [`ObsErrorCode::ObsInvalidMetric`]
/// - any field exceeds [`PER_FIELD_CHAR_CAP`] characters — surfaces
///   [`ObsErrorCode::ObsCardinalityGuard`] with the offending field
///   name in [`ObsError::dim_overflow`]
///
/// Quantitative caps ([`MAX_DISTINCT_TENANTS_GLOBAL`],
/// [`MAX_DISTINCT_REPOS_PER_TENANT`]) live on
/// [`crate::cardinality_guard::CardinalityGuard`] — they are enforced
/// stateful, dimension-validation is per-sample.
pub fn validate_dimensions(d: &Dimensions) -> Result<(), ObsError> {
    if d.ticket_id.is_empty() {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            "ticket_id must not be empty",
        ));
    }
    if d.wave_id.is_empty() {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            "wave_id must not be empty",
        ));
    }
    if d.tenant_id.is_empty() {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            "tenant_id must not be empty",
        ));
    }
    if d.repo_id.is_empty() {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            "repo_id must not be empty",
        ));
    }
    check_cap("ticket_id", &d.ticket_id)?;
    check_cap("wave_id", &d.wave_id)?;
    check_cap("tenant_id", &d.tenant_id)?;
    check_cap("repo_id", &d.repo_id)?;
    Ok(())
}

fn check_cap(field: &'static str, value: &str) -> Result<(), ObsError> {
    if value.chars().count() > PER_FIELD_CHAR_CAP {
        return Err(ObsError::cardinality(
            field,
            format!("field exceeds {PER_FIELD_CHAR_CAP}-char cap"),
        ));
    }
    Ok(())
}

impl serde::Serialize for Dimensions {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("Dimensions", 5)?;
        st.serialize_field("ticket_id", self.ticket_id.as_ref())?;
        st.serialize_field("wave_id", self.wave_id.as_ref())?;
        st.serialize_field("tenant_id", self.tenant_id.as_ref())?;
        st.serialize_field("repo_id", self.repo_id.as_ref())?;
        st.serialize_field("generation_id", &self.generation_id)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for Dimensions {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        #[expect(
            clippy::enum_variant_names,
            reason = "variant suffix mirrors the spec-locked Dimensions field names"
        )]
        enum Field {
            TicketId,
            WaveId,
            TenantId,
            RepoId,
            GenerationId,
        }
        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("Dimensions field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "ticket_id" => Ok(Field::TicketId),
                            "wave_id" => Ok(Field::WaveId),
                            "tenant_id" => Ok(Field::TenantId),
                            "repo_id" => Ok(Field::RepoId),
                            "generation_id" => Ok(Field::GenerationId),
                            other => Err(E::unknown_field(
                                other,
                                &[
                                    "ticket_id",
                                    "wave_id",
                                    "tenant_id",
                                    "repo_id",
                                    "generation_id",
                                ],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct DV;
        impl<'d> serde::de::Visitor<'d> for DV {
            type Value = Dimensions;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Dimensions struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<Dimensions, A::Error> {
                let mut ticket_id: Option<Box<str>> = None;
                let mut wave_id: Option<Box<str>> = None;
                let mut tenant_id: Option<Box<str>> = None;
                let mut repo_id: Option<Box<str>> = None;
                let mut generation_id: Option<u64> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::TicketId => {
                            if ticket_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("ticket_id"));
                            }
                            let v: String = map.next_value()?;
                            ticket_id = Some(v.into_boxed_str());
                        }
                        Field::WaveId => {
                            if wave_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("wave_id"));
                            }
                            let v: String = map.next_value()?;
                            wave_id = Some(v.into_boxed_str());
                        }
                        Field::TenantId => {
                            if tenant_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("tenant_id"));
                            }
                            let v: String = map.next_value()?;
                            tenant_id = Some(v.into_boxed_str());
                        }
                        Field::RepoId => {
                            if repo_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("repo_id"));
                            }
                            let v: String = map.next_value()?;
                            repo_id = Some(v.into_boxed_str());
                        }
                        Field::GenerationId => {
                            if generation_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation_id"));
                            }
                            generation_id = Some(map.next_value()?);
                        }
                    }
                }
                Ok(Dimensions {
                    ticket_id: ticket_id
                        .ok_or_else(|| serde::de::Error::missing_field("ticket_id"))?,
                    wave_id: wave_id.ok_or_else(|| serde::de::Error::missing_field("wave_id"))?,
                    tenant_id: tenant_id
                        .ok_or_else(|| serde::de::Error::missing_field("tenant_id"))?,
                    repo_id: repo_id.ok_or_else(|| serde::de::Error::missing_field("repo_id"))?,
                    generation_id: generation_id
                        .ok_or_else(|| serde::de::Error::missing_field("generation_id"))?,
                })
            }
        }

        de.deserialize_struct(
            "Dimensions",
            &[
                "ticket_id",
                "wave_id",
                "tenant_id",
                "repo_id",
                "generation_id",
            ],
            DV,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dimensions, MAX_DISTINCT_REPOS_PER_TENANT, MAX_DISTINCT_TENANTS_GLOBAL, PER_FIELD_CHAR_CAP,
        validate_dimensions,
    };
    use crate::errors::ObsErrorCode;

    fn d() -> Dimensions {
        Dimensions::new("OBS-01", "8", "t1", "r1", 1)
    }

    #[test]
    fn caps_match_spec() {
        assert_eq!(PER_FIELD_CHAR_CAP, 256);
        assert_eq!(MAX_DISTINCT_TENANTS_GLOBAL, 100_000);
        assert_eq!(MAX_DISTINCT_REPOS_PER_TENANT, 10_000);
    }

    #[test]
    fn valid_dimensions_accepted() {
        match validate_dimensions(&d()) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn empty_ticket_id_rejected() {
        let mut x = d();
        x.ticket_id = Box::from("");
        match validate_dimensions(&x) {
            Ok(()) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, ObsErrorCode::ObsInvalidMetric),
        }
    }

    #[test]
    fn empty_wave_id_rejected() {
        let mut x = d();
        x.wave_id = Box::from("");
        match validate_dimensions(&x) {
            Ok(()) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, ObsErrorCode::ObsInvalidMetric),
        }
    }

    #[test]
    fn empty_tenant_id_rejected() {
        let mut x = d();
        x.tenant_id = Box::from("");
        match validate_dimensions(&x) {
            Ok(()) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, ObsErrorCode::ObsInvalidMetric),
        }
    }

    #[test]
    fn empty_repo_id_rejected() {
        let mut x = d();
        x.repo_id = Box::from("");
        match validate_dimensions(&x) {
            Ok(()) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, ObsErrorCode::ObsInvalidMetric),
        }
    }

    #[test]
    fn over_cap_field_rejected_with_overflow_tag() {
        let mut x = d();
        let big = "a".repeat(PER_FIELD_CHAR_CAP.saturating_add(1));
        x.tenant_id = big.into_boxed_str();
        match validate_dimensions(&x) {
            Ok(()) => assert!(false, "must fail"),
            Err(e) => {
                assert_eq!(e.code, ObsErrorCode::ObsCardinalityGuard);
                assert_eq!(e.dim_overflow.as_deref(), Some("tenant_id"));
            }
        }
    }

    #[test]
    fn at_cap_field_accepted() {
        let mut x = d();
        let big = "a".repeat(PER_FIELD_CHAR_CAP);
        x.tenant_id = big.into_boxed_str();
        match validate_dimensions(&x) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn dimensions_serde_roundtrip() {
        let x = d();
        let buf = match serde_json::to_vec(&x) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "ser: {e}");
                return;
            }
        };
        match serde_json::from_slice::<Dimensions>(&buf) {
            Ok(got) => assert_eq!(got, x),
            Err(e) => assert!(false, "de: {e}"),
        }
    }
}
