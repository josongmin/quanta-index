//! Admission for fixed-field publication metadata shared by build and read paths.

use super::{
    CoverageBinding, IndexError, MemberBinding, MemoryLifetime, OwnerCoverage, OwnerMemberBinding,
    ProjectionBinding, ProjectionCertificate, ProjectionOwnerContribution, WorkBudget,
};

pub const MAX_BINDING_METADATA_BYTES: usize = 16 * 1024;

// The typed entrypoints below return fixed identities/scalars, not escaping
// heap data. This parser allowance is logical custody, not an allocator/RSS cap.
fn decode<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    label: &'static str,
    budget: &WorkBudget,
) -> Result<T, IndexError> {
    budget.checkpoint()?;
    if bytes.len() > MAX_BINDING_METADATA_BYTES {
        return Err(IndexError::CorruptObject(format!(
            "{label} exceeds fixed-field bound"
        )));
    }
    let allowance = bytes
        .len()
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let decoded = serde_json::from_slice(bytes)
        .map_err(|error| IndexError::CorruptObject(format!("{label}: {error}")));
    budget.checkpoint()?;
    decoded
}

macro_rules! fixed_binding {
    ($read:ident, $ty:ty, $label:literal) => {
        pub fn $read(bytes: &[u8], budget: &WorkBudget) -> Result<$ty, IndexError> {
            decode(bytes, $label, budget)
        }
    };
}

fixed_binding!(decode_coverage_binding, CoverageBinding, "coverage binding");
fixed_binding!(decode_owner_coverage, OwnerCoverage, "owner coverage");
fixed_binding!(decode_member_binding, MemberBinding, "member binding");
fixed_binding!(
    decode_owner_member_binding,
    OwnerMemberBinding,
    "owner member"
);
fixed_binding!(
    decode_projection_binding,
    ProjectionBinding,
    "projection binding"
);
fixed_binding!(
    decode_projection_certificate,
    ProjectionCertificate,
    "projection certificate"
);
fixed_binding!(
    decode_projection_owner,
    ProjectionOwnerContribution,
    "projection contribution"
);
