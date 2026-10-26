//! Policy arithmetic for lexical maintenance.
//!
//! Run membership and encoded byte
//! summaries must be authenticated by the lexical producer and seal verifier;
//! this module neither persists those facts nor schedules work.

use sha2::{Digest as _, Sha256};

use crate::error::CoreError;
use crate::ingest_resource::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE;

/// Maximum live run headers examined by one hard-pressure recovery attempt.
/// This fixed implementation limit is part of the supported policy range.
pub const MAX_HARD_SCAN_RUNS: u64 = 4_096;

/// Externally supplied limits for one lexical authority's logical runs and
/// attributed index bundles (descriptors, segment maps and posting objects).
///
/// Source bodies, global routes and control files belong to service resource
/// accounting rather than these logical run totals. A shared object can be
/// attributed once to each owning segment without claiming unique disk usage.
///
/// The policy digest identifies these configuration values. It is not evidence
/// that thresholds were derived from a supported workload or that physical
/// memory, I/O, and disk capacity has been reserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalMaintenancePolicy {
    soft_runs: u64,
    hard_runs: u64,
    soft_active_encoded_bytes: u64,
    hard_active_encoded_bytes: u64,
    max_merge_fan_in: u32,
    max_merge_input_encoded_bytes: u64,
    max_staging_bytes: u64,
    max_staging_age_millis: u64,
}

impl LexicalMaintenancePolicy {
    pub fn new(
        soft_runs: u64,
        hard_runs: u64,
        soft_active_encoded_bytes: u64,
        hard_active_encoded_bytes: u64,
        max_merge_fan_in: u32,
        max_merge_input_encoded_bytes: u64,
        max_staging_bytes: u64,
        max_staging_age_millis: u64,
    ) -> Result<Self, CoreError> {
        if soft_runs == 0
            || soft_active_encoded_bytes == 0
            || max_staging_bytes == 0
            || max_staging_age_millis == 0
            || max_merge_input_encoded_bytes == 0
            || max_merge_fan_in < 2
            || soft_runs > hard_runs
            || hard_runs > MAX_HARD_SCAN_RUNS
            || soft_active_encoded_bytes > hard_active_encoded_bytes
        {
            return Err(invalid("invalid soft/hard, fan-in, or staging limits"));
        }
        Ok(Self {
            soft_runs,
            hard_runs,
            soft_active_encoded_bytes,
            hard_active_encoded_bytes,
            max_merge_fan_in,
            max_merge_input_encoded_bytes,
            max_staging_bytes,
            max_staging_age_millis,
        })
    }

    /// Configuration identity for staged packets and attempt checkpoints. No durable
    /// policy authority or benchmark provenance is implied by this hash.
    ///
    /// Hashing is infallible by construction: it uses only fixed-width fields.
    #[must_use]
    pub fn policy_sha256(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-lexical-maintenance-policy-v1\0");
        for value in [
            self.soft_runs,
            self.hard_runs,
            self.soft_active_encoded_bytes,
            self.hard_active_encoded_bytes,
            u64::from(self.max_merge_fan_in),
            self.max_merge_input_encoded_bytes,
            self.max_staging_bytes,
            self.max_staging_age_millis,
        ] {
            hasher.update(value.to_le_bytes());
        }
        hasher.finalize().into()
    }

    /// Whether either logical run or active encoded-byte count has reached its
    /// soft limit. Candidate availability and capacity remain separate checks.
    #[must_use]
    pub fn merge_due(&self, current: TargetCounters) -> bool {
        current.runs >= self.soft_runs
            || current.active_encoded_bytes >= self.soft_active_encoded_bytes
    }

    /// A hard-pressure publication may need to inspect the complete bounded
    /// old run frontier before deciding whether an append can be admitted.
    #[must_use]
    pub fn hard_pressure(&self, current: TargetCounters) -> bool {
        current.runs >= self.hard_runs
            || current.active_encoded_bytes >= self.hard_active_encoded_bytes
    }

    /// Check the post-merge target, allowing recovery from an already exceeded
    /// limit only when that particular dimension strictly improves.
    pub fn admit_target(
        &self,
        base: TargetCounters,
        target: TargetCounters,
    ) -> Result<(), CoreError> {
        admit_dimension("runs", base.runs, target.runs, self.hard_runs)?;
        admit_dimension(
            "active encoded bytes",
            base.active_encoded_bytes,
            target.active_encoded_bytes,
            self.hard_active_encoded_bytes,
        )
    }

    /// Admit a cursor-only maintenance publication after the caller has
    /// independently proved that both authority roots differ only in their
    /// authenticated selection cursors.
    ///
    /// This exception cannot admit a changed
    /// run count, segment count, or attributed index byte count. It permits a
    /// bounded scan to advance when a newly configured hard limit is already
    /// below the sealed base and no merge is available in the current window.
    pub fn admit_cursor_only_target(
        &self,
        base: TargetCounters,
        target: TargetCounters,
    ) -> Result<(), CoreError> {
        if base != target {
            return Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                message: "lexical maintenance cursor-only target changes counters".into(),
            });
        }
        if base.runs <= self.hard_runs
            && base.active_encoded_bytes <= self.hard_active_encoded_bytes
        {
            return self.admit_target(base, target);
        }
        Ok(())
    }

    /// Validate an in-memory packet against its pinned input root, config,
    /// bounded payload, fan-in and monotonic age before publication adopts it.
    pub fn check_staged_packet(
        &self,
        identity: StagedPacketIdentity,
        expected_base_sha256: [u8; 32],
        bounds: StagedPacketBounds,
        now_monotonic_millis: u64,
    ) -> Result<(), StagedPacketRefusal> {
        if identity.base_sha256 != expected_base_sha256 {
            return Err(StagedPacketRefusal::BaseChanged);
        }
        if identity.policy_sha256 != self.policy_sha256() {
            return Err(StagedPacketRefusal::PolicyChanged);
        }
        if bounds.input_runs == 0 || bounds.input_runs > self.max_merge_fan_in {
            return Err(StagedPacketRefusal::FanInOutOfRange);
        }
        if bounds.encoded_bytes == 0 {
            return Err(StagedPacketRefusal::EmptyPayload);
        }
        if bounds.encoded_bytes > self.max_staging_bytes {
            return Err(StagedPacketRefusal::BytesExceeded);
        }
        let age = now_monotonic_millis
            .checked_sub(bounds.created_at_monotonic_millis)
            .ok_or(StagedPacketRefusal::ClockReversed)?;
        if age > self.max_staging_age_millis {
            return Err(StagedPacketRefusal::AgeExceeded);
        }
        Ok(())
    }

    #[must_use]
    pub const fn max_merge_fan_in(&self) -> u32 {
        self.max_merge_fan_in
    }

    /// Maximum attributed index bytes of complete input cohorts selected for
    /// one merge. Body residency and output space require service admission.
    #[must_use]
    pub const fn max_merge_input_encoded_bytes(&self) -> u64 {
        self.max_merge_input_encoded_bytes
    }

    /// Maximum encoded payload retained by optional merge preparation. This
    /// is a logical packet limit, independent of process resident memory.
    #[must_use]
    pub const fn max_staging_bytes(&self) -> u64 {
        self.max_staging_bytes
    }
}

/// One logical run, which may contain multiple physical segments. The caller
/// must authenticate the segment set, uniqueness, and encoded-byte total.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunSummary {
    segments: u64,
    active_encoded_bytes: u64,
}

impl RunSummary {
    pub fn new(segments: u64, active_encoded_bytes: u64) -> Result<Self, CoreError> {
        if segments == 0 || active_encoded_bytes == 0 {
            return Err(invalid("one run requires segments and encoded bytes"));
        }
        Ok(Self {
            segments,
            active_encoded_bytes,
        })
    }

    #[must_use]
    pub const fn segments(&self) -> u64 {
        self.segments
    }

    #[must_use]
    pub const fn active_encoded_bytes(&self) -> u64 {
        self.active_encoded_bytes
    }
}

/// Authenticated target totals supplied by the lexical producer/verifier.
/// Segment count is reported separately and is never used as run count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetCounters {
    runs: u64,
    segments: u64,
    active_encoded_bytes: u64,
}

impl TargetCounters {
    pub fn new(runs: u64, segments: u64, active_encoded_bytes: u64) -> Result<Self, CoreError> {
        if (runs == 0 && (segments != 0 || active_encoded_bytes != 0))
            || (runs != 0 && (segments < runs || active_encoded_bytes == 0))
        {
            return Err(invalid("run, segment, and encoded-byte totals disagree"));
        }
        Ok(Self {
            runs,
            segments,
            active_encoded_bytes,
        })
    }

    /// Compute the target from explicitly selected input and output runs.
    /// Arithmetic does not prove that those runs belong to the sealed root.
    pub fn checked_transition(
        self,
        removed: &[RunSummary],
        added: &[RunSummary],
    ) -> Result<Self, CoreError> {
        let removed_runs = u64::try_from(removed.len())
            .map_err(|_error| invalid("removed run count exceeds u64"))?;
        let added_runs =
            u64::try_from(added.len()).map_err(|_error| invalid("added run count exceeds u64"))?;
        let removed_segments = sum_run_field(removed, RunSummary::segments)?;
        let added_segments = sum_run_field(added, RunSummary::segments)?;
        let removed_bytes = sum_run_field(removed, RunSummary::active_encoded_bytes)?;
        let added_bytes = sum_run_field(added, RunSummary::active_encoded_bytes)?;
        let runs = self
            .runs
            .checked_sub(removed_runs)
            .and_then(|value| value.checked_add(added_runs))
            .ok_or_else(|| invalid("target run count underflows or overflows"))?;
        let segments = self
            .segments
            .checked_sub(removed_segments)
            .and_then(|value| value.checked_add(added_segments))
            .ok_or_else(|| invalid("target segment count underflows or overflows"))?;
        let bytes = self
            .active_encoded_bytes
            .checked_sub(removed_bytes)
            .and_then(|value| value.checked_add(added_bytes))
            .ok_or_else(|| invalid("target encoded-byte count underflows or overflows"))?;
        Self::new(runs, segments, bytes)
    }

    #[must_use]
    pub const fn runs(&self) -> u64 {
        self.runs
    }

    #[must_use]
    pub const fn segments(&self) -> u64 {
        self.segments
    }

    #[must_use]
    pub const fn active_encoded_bytes(&self) -> u64 {
        self.active_encoded_bytes
    }
}

/// A staged packet is valid only for this exact authenticated base and config.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StagedPacketIdentity {
    base_sha256: [u8; 32],
    policy_sha256: [u8; 32],
}

impl StagedPacketIdentity {
    pub fn new(base_sha256: [u8; 32], policy_sha256: [u8; 32]) -> Result<Self, CoreError> {
        if base_sha256 == [0; 32] || policy_sha256 == [0; 32] {
            return Err(invalid("staged packet identity has an empty digest"));
        }
        Ok(Self {
            base_sha256,
            policy_sha256,
        })
    }
}

/// The packet's resident encoded bytes and age are checked inclusively.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StagedPacketBounds {
    pub encoded_bytes: u64,
    pub created_at_monotonic_millis: u64,
    pub input_runs: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedPacketRefusal {
    BaseChanged,
    PolicyChanged,
    FanInOutOfRange,
    EmptyPayload,
    BytesExceeded,
    ClockReversed,
    AgeExceeded,
}

fn sum_run_field(runs: &[RunSummary], field: fn(&RunSummary) -> u64) -> Result<u64, CoreError> {
    runs.iter().try_fold(0_u64, |sum, run| {
        sum.checked_add(field(run))
            .ok_or_else(|| invalid("run summary total overflows"))
    })
}

fn admit_dimension(name: &str, base: u64, target: u64, hard: u64) -> Result<(), CoreError> {
    if target <= hard || (base > hard && target < base) {
        return Ok(());
    }
    Err(CoreError::Typed {
        code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
        message: format!(
            "lexical maintenance target {name} {target} exceeds hard limit {hard} without improving base {base}"
        ),
    })
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical maintenance: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::{
        LexicalMaintenancePolicy, MAX_HARD_SCAN_RUNS, RunSummary, StagedPacketBounds,
        StagedPacketIdentity, StagedPacketRefusal, TargetCounters,
    };
    use crate::error::CoreError;
    use crate::ingest_resource::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE;

    fn policy() -> LexicalMaintenancePolicy {
        LexicalMaintenancePolicy::new(8, 10, 80, 100, 2, 200, 50, 10).expect("valid policy")
    }

    fn counters(runs: u64, bytes: u64) -> TargetCounters {
        TargetCounters::new(runs, runs, bytes).expect("valid counters")
    }

    #[test]
    fn policy_requires_coherent_explicit_bounds() {
        assert!(LexicalMaintenancePolicy::new(10, 10, 100, 100, 2, 1, 1, 1).is_ok());
        assert!(
            LexicalMaintenancePolicy::new(1, MAX_HARD_SCAN_RUNS + 1, 1, 100, 2, 1, 1, 1).is_err()
        );
        assert!(!policy().hard_pressure(counters(9, 99)));
        assert!(policy().hard_pressure(counters(10, 99)));
        assert!(policy().hard_pressure(counters(9, 100)));
        for args in [
            (0, 10, 1, 100, 2, 1, 1, 1),
            (11, 10, 1, 100, 2, 1, 1, 1),
            (1, 10, 0, 100, 2, 1, 1, 1),
            (1, 10, 101, 100, 2, 1, 1, 1),
            (1, 10, 1, 100, 1, 1, 1, 1),
            (1, 10, 1, 100, 2, 0, 1, 1),
            (1, 10, 1, 100, 2, 1, 0, 1),
            (1, 10, 1, 100, 2, 1, 1, 0),
        ] {
            assert!(
                LexicalMaintenancePolicy::new(
                    args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7
                )
                .is_err(),
                "invalid policy accepted: {args:?}"
            );
        }
        assert!(!policy().merge_due(counters(7, 79)));
        assert!(policy().merge_due(counters(8, 79)));
        assert!(policy().merge_due(counters(7, 80)));
    }

    #[test]
    fn target_admission_uses_post_merge_values_and_per_dimension_recovery() {
        let cases = [
            (10, 100, 10, 100, true),  // Exactly at both limits.
            (11, 110, 10, 100, true),  // Prepared merge restores both.
            (12, 120, 11, 110, true),  // Both still high but improve.
            (11, 110, 11, 110, false), // Neither improves.
            (11, 110, 12, 120, false), // Both grow.
            (10, 100, 11, 101, false), // New breach.
            (12, 120, 11, 121, false), // Runs improve but bytes worsen.
            (12, 120, 13, 119, false), // Bytes improve but runs worsen.
        ];
        for (base_runs, base_bytes, target_runs, target_bytes, admitted) in cases {
            let result = policy().admit_target(
                counters(base_runs, base_bytes),
                counters(target_runs, target_bytes),
            );
            if admitted {
                assert!(
                    result.is_ok(),
                    "case: {base_runs}, {base_bytes} -> {target_runs}, {target_bytes}: {result:?}"
                );
            } else {
                assert!(
                    matches!(&result, Err(CoreError::Typed { code, .. }) if *code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE),
                    "case: {base_runs}, {base_bytes} -> {target_runs}, {target_bytes}: {result:?}"
                );
            }
        }
    }

    #[test]
    fn cursor_only_admission_requires_exactly_equal_counters() {
        let policy = policy();
        assert!(
            policy
                .admit_target(counters(11, 110), counters(11, 110))
                .is_err()
        );
        assert!(
            policy
                .admit_cursor_only_target(counters(11, 110), counters(11, 110))
                .is_ok()
        );
        for target in [counters(12, 110), counters(11, 111), counters(10, 110)] {
            assert!(
                policy
                    .admit_cursor_only_target(counters(11, 110), target)
                    .is_err()
            );
        }
    }

    #[test]
    fn run_transition_checks_arithmetic_without_equating_runs_to_segments() {
        let base = TargetCounters::new(2, 5, 30).expect("base");
        let removed = [RunSummary::new(2, 12).expect("run")];
        let added = [RunSummary::new(1, 8).expect("merged run")];
        let target = base.checked_transition(&removed, &added).expect("target");
        assert_eq!(
            (
                target.runs(),
                target.segments(),
                target.active_encoded_bytes()
            ),
            (2, 4, 26)
        );
        assert!(TargetCounters::new(1, 0, 1).is_err());
        assert!(RunSummary::new(0, 1).is_err());
        assert!(
            TargetCounters::new(u64::MAX, u64::MAX, u64::MAX)
                .expect("max counters")
                .checked_transition(&[], &added)
                .is_err()
        );
        assert!(
            base.checked_transition(&[RunSummary::new(6, 31).expect("oversized run")], &[])
                .is_err()
        );
    }

    #[test]
    fn staged_packet_bounds_are_inclusive_and_bound_to_base_and_policy() {
        let policy = policy();
        let base = [7; 32];
        let identity = StagedPacketIdentity::new(base, policy.policy_sha256()).expect("identity");
        let bounds = StagedPacketBounds {
            encoded_bytes: 50,
            created_at_monotonic_millis: 100,
            input_runs: 2,
        };
        assert_eq!(
            policy.check_staged_packet(identity, base, bounds, 110),
            Ok(())
        );
        assert_eq!(
            policy.check_staged_packet(identity, [8; 32], bounds, 110),
            Err(StagedPacketRefusal::BaseChanged)
        );
        let changed =
            LexicalMaintenancePolicy::new(8, 10, 80, 100, 2, 200, 51, 10).expect("changed");
        assert_ne!(policy.policy_sha256(), changed.policy_sha256());
        assert_eq!(
            changed.check_staged_packet(identity, base, bounds, 110),
            Err(StagedPacketRefusal::PolicyChanged)
        );
        for (altered, now, reason) in [
            (
                StagedPacketBounds {
                    input_runs: 0,
                    ..bounds
                },
                110,
                StagedPacketRefusal::FanInOutOfRange,
            ),
            (
                StagedPacketBounds {
                    input_runs: 3,
                    ..bounds
                },
                110,
                StagedPacketRefusal::FanInOutOfRange,
            ),
            (
                StagedPacketBounds {
                    encoded_bytes: 0,
                    ..bounds
                },
                110,
                StagedPacketRefusal::EmptyPayload,
            ),
            (
                StagedPacketBounds {
                    encoded_bytes: 51,
                    ..bounds
                },
                110,
                StagedPacketRefusal::BytesExceeded,
            ),
            (bounds, 99, StagedPacketRefusal::ClockReversed),
            (bounds, 111, StagedPacketRefusal::AgeExceeded),
        ] {
            assert_eq!(
                policy.check_staged_packet(identity, base, altered, now),
                Err(reason)
            );
        }
    }
}
