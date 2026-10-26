//! Explicit Legacy regular-file capacity profile; no deployment defaults.

use std::collections::BTreeMap;

use anyhow::Result;

use super::{EnvLookup, required_positive_raw_u64};

/// All 23 knobs belong to one all-or-none daemon policy family.
pub(super) const ENV_VARS: &[&str] = &[
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS",
    "QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES",
    "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES",
];

/// Upper bounds for the independent boot census of regular-file inodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPhysicalInventoryLimits {
    pub bytes: u64,
    pub files: u64,
    pub pending: u64,
    pub entries: u64,
    pub depth: u32,
    pub pending_frames: u32,
}

/// Registry and physical reader limits for one leased root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPhysicalBinderLimits {
    pub reader_handles: u32,
    pub tracked_entries: u32,
    pub process_roots: u32,
}

/// One independently held physical parent, in bytes and regular-file slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPhysicalAllowance {
    pub bytes: u64,
    pub files: u64,
}

/// Deployment-supplied Legacy physical limits.
///
/// The composition root converts
/// these values into its existing inventory, binder, and allowance types.
/// This contract does not cover disk blocks, RSS, or process-wide FD limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPublicationPhysicalPolicy {
    pub inventory: LegacyPhysicalInventoryLimits,
    pub binder: LegacyPhysicalBinderLimits,
    pub global_sqlite_handles: u32,
    pub maintenance: LegacyPhysicalAllowance,
    pub control: LegacyPhysicalAllowance,
    pub work: LegacyPhysicalAllowance,
    pub history: LegacyPhysicalAllowance,
    pub durable_control_file_bytes: u64,
    pub repomap_control_file_bytes: u64,
    pub repomap_candidate_file_bytes: u64,
    pub catalog_parent: LegacyPhysicalAllowance,
}

/// Explicit fixture limits for the named in-process test/harness constructor.
///
/// These values are test inputs, not measured deployment limits. Production
/// configuration obtains every physical limit from the all-or-none env family.
pub const TEST_STATE_ROOT_PHYSICAL_POLICY: LegacyPublicationPhysicalPolicy =
    LegacyPublicationPhysicalPolicy {
        inventory: LegacyPhysicalInventoryLimits {
            bytes: 16_u64 << 30,
            files: 200_000,
            pending: 100_000,
            entries: 400_000,
            depth: 64,
            pending_frames: 4096,
        },
        binder: LegacyPhysicalBinderLimits {
            reader_handles: 4096,
            tracked_entries: 100_000,
            process_roots: 256,
        },
        global_sqlite_handles: 8192,
        maintenance: LegacyPhysicalAllowance {
            bytes: 1_u64 << 30,
            files: 1000,
        },
        control: LegacyPhysicalAllowance {
            bytes: 1_u64 << 30,
            files: 10_000,
        },
        work: LegacyPhysicalAllowance {
            bytes: 4_u64 << 30,
            files: 30_000,
        },
        history: LegacyPhysicalAllowance {
            bytes: 1_u64 << 30,
            files: 10_000,
        },
        durable_control_file_bytes: 1_u64 << 20,
        repomap_control_file_bytes: 64_u64 << 20,
        repomap_candidate_file_bytes: 512_u64 << 20,
        catalog_parent: LegacyPhysicalAllowance {
            bytes: 2_u64 << 30,
            files: 10_000,
        },
    };

impl LegacyPublicationPhysicalPolicy {
    /// Reject impossible capacity windows before opening a leased state root.
    pub fn validate(self) -> Result<Self> {
        let inventory = self.inventory;
        anyhow::ensure!(
            inventory.bytes > 0
                && inventory.files > 0
                && inventory.pending > 0
                && inventory.entries > 0
                && inventory.depth > 0
                && inventory.pending_frames > 0,
            "Legacy physical inventory requires positive limits"
        );
        let binder = self.binder;
        anyhow::ensure!(
            binder.reader_handles > 0
                && binder.tracked_entries > 0
                && binder.process_roots > 0
                && self.global_sqlite_handles >= binder.reader_handles,
            "Legacy physical binder requires positive limits and a SQLite ceiling covering reader handles"
        );
        for (name, parent) in [
            ("maintenance", self.maintenance),
            ("control", self.control),
            ("work", self.work),
            ("history", self.history),
            ("Catalog", self.catalog_parent),
        ] {
            anyhow::ensure!(
                parent.bytes > 0
                    && parent.files > 0
                    && parent.bytes <= inventory.bytes
                    && parent.files <= inventory.files,
                "Legacy physical {name} parent must be positive and within the global byte/file caps"
            );
        }
        anyhow::ensure!(
            self.durable_control_file_bytes > 0
                && self.durable_control_file_bytes <= inventory.bytes,
            "Legacy durable Control file bound must be positive and within the global byte cap"
        );
        anyhow::ensure!(
            self.repomap_control_file_bytes > 0
                && self.repomap_control_file_bytes <= inventory.bytes
                && self.repomap_candidate_file_bytes > 0
                && self.repomap_candidate_file_bytes <= inventory.bytes,
            "Legacy RepoMap per-file bounds must be positive and within the global byte cap"
        );
        // Maintenance, Catalog, and History parents can remain held while an
        // event holds Control and Work. Reserve arithmetic must be bounded.
        let concurrent_bytes = [
            self.maintenance.bytes,
            self.catalog_parent.bytes,
            self.control.bytes,
            self.work.bytes,
            self.history.bytes,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(|| anyhow::anyhow!("Legacy physical concurrent parent bytes overflow"))?;
        let concurrent_files = [
            self.maintenance.files,
            self.catalog_parent.files,
            self.control.files,
            self.work.files,
            self.history.files,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(|| anyhow::anyhow!("Legacy physical concurrent parent files overflow"))?;
        anyhow::ensure!(
            concurrent_bytes <= inventory.bytes && concurrent_files <= inventory.files,
            "Legacy physical concurrently held parents exceed the global byte/file caps"
        );
        Ok(self)
    }
}

pub(super) fn from_lookup(
    lookup: &EnvLookup<'_>,
) -> Result<Option<LegacyPublicationPhysicalPolicy>> {
    let raw = ENV_VARS
        .iter()
        .map(|name| lookup(name).map(|value| (*name, value)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if raw.values().all(Option::is_none) {
        return Ok(None);
    }
    let get = |name: &str| -> Result<u64> {
        required_positive_raw_u64(name, raw.get(name).cloned().flatten())
    };
    let get_u32 = |name: &str| -> Result<u32> {
        let value = get(name)?;
        let converted =
            u32::try_from(value).map_err(|error| anyhow::anyhow!("{name} exceeds u32: {error}"))?;
        Ok(converted)
    };
    let policy = LegacyPublicationPhysicalPolicy {
        inventory: LegacyPhysicalInventoryLimits {
            bytes: get("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES")?,
            files: get("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES")?,
            pending: get("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING")?,
            entries: get("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES")?,
            depth: get_u32("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH")?,
            pending_frames: get_u32("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES")?,
        },
        binder: LegacyPhysicalBinderLimits {
            reader_handles: get_u32("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES")?,
            tracked_entries: get_u32("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES")?,
            process_roots: get_u32("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS")?,
        },
        global_sqlite_handles: get_u32("QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES")?,
        maintenance: LegacyPhysicalAllowance {
            bytes: get("QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES")?,
            files: get("QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES")?,
        },
        control: LegacyPhysicalAllowance {
            bytes: get("QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES")?,
            files: get("QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES")?,
        },
        work: LegacyPhysicalAllowance {
            bytes: get("QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES")?,
            files: get("QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES")?,
        },
        history: LegacyPhysicalAllowance {
            bytes: get("QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES")?,
            files: get("QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES")?,
        },
        durable_control_file_bytes: get(
            "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES",
        )?,
        repomap_control_file_bytes: get(
            "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
        )?,
        repomap_candidate_file_bytes: get(
            "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
        )?,
        catalog_parent: LegacyPhysicalAllowance {
            bytes: get("QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES")?,
            files: get("QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES")?,
        },
    };
    policy.validate().map(Some)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::app::config::SearchdConfig;

    fn valid_values() -> BTreeMap<&'static str, &'static str> {
        BTreeMap::from([
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES", "1048576"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES", "128"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING", "64"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES", "512"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH", "12"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES", "24"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES", "16"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES", "256"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS", "4"),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES",
                "32",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES",
                "4096",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES",
                "2",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES",
                "8192",
            ),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES", "4"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES", "16384"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES", "8"),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES",
                "8192",
            ),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES", "3"),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES",
                "40",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
                "72",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
                "4096",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES",
                "32768",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES",
                "6",
            ),
        ])
    }

    fn configured(
        values: &BTreeMap<&str, &str>,
    ) -> Result<Option<LegacyPublicationPhysicalPolicy>> {
        from_lookup(&|name| Ok(values.get(name).map(|value| (*value).to_string())))
    }

    #[test]
    fn all_twenty_three_limits_are_explicit_and_preserved_without_defaults() -> Result<()> {
        anyhow::ensure!(ENV_VARS.len() == 23, "physical family lost a required knob");
        anyhow::ensure!(
            from_lookup(&|_name| Ok(None))?.is_none(),
            "absent physical family acquired implicit limits"
        );
        let base = SearchdConfig::from_test_state_root(PathBuf::from("/tmp/qi-physical-config"));
        anyhow::ensure!(
            base.legacy_publication_physical_policy() == Some(TEST_STATE_ROOT_PHYSICAL_POLICY),
            "named test builder lost its explicit physical fixture"
        );
        anyhow::ensure!(
            SearchdConfig::base_with_state_root(PathBuf::from("/tmp/qi-physical-config"))
                .legacy_publication_physical_policy()
                .is_none(),
            "production base acquired fixture limits before env parsing"
        );
        let values = valid_values();
        let policy =
            configured(&values)?.ok_or_else(|| anyhow::anyhow!("complete family missing"))?;
        let expected = LegacyPublicationPhysicalPolicy {
            inventory: LegacyPhysicalInventoryLimits {
                bytes: 1_048_576,
                files: 128,
                pending: 64,
                entries: 512,
                depth: 12,
                pending_frames: 24,
            },
            binder: LegacyPhysicalBinderLimits {
                reader_handles: 16,
                tracked_entries: 256,
                process_roots: 4,
            },
            global_sqlite_handles: 32,
            maintenance: LegacyPhysicalAllowance {
                bytes: 4096,
                files: 2,
            },
            control: LegacyPhysicalAllowance {
                bytes: 8192,
                files: 4,
            },
            work: LegacyPhysicalAllowance {
                bytes: 16_384,
                files: 8,
            },
            history: LegacyPhysicalAllowance {
                bytes: 8192,
                files: 3,
            },
            durable_control_file_bytes: 40,
            repomap_control_file_bytes: 72,
            repomap_candidate_file_bytes: 4096,
            catalog_parent: LegacyPhysicalAllowance {
                bytes: 32_768,
                files: 6,
            },
        };
        anyhow::ensure!(policy == expected, "physical knob mapping changed");
        let applied = base.try_with_legacy_publication_physical_policy(Some(policy))?;
        anyhow::ensure!(
            applied.legacy_publication_physical_policy() == Some(expected),
            "physical config builder dropped a validated limit"
        );
        Ok(())
    }

    #[test]
    fn partial_zero_and_unrepresentable_limits_fail_before_composition() -> Result<()> {
        let values = valid_values();
        for missing in ENV_VARS {
            let mut partial = values.clone();
            let _removed = partial.remove(*missing);
            let error = configured(&partial).err().ok_or_else(|| {
                anyhow::anyhow!("missing {missing} did not fail physical parsing")
            })?;
            anyhow::ensure!(
                error.to_string().contains(*missing),
                "wrong missing knob: {error}"
            );
            let mut zero = values.clone();
            let _old = zero.insert(*missing, "0");
            anyhow::ensure!(configured(&zero).is_err(), "zero {missing} was accepted");
        }
        let mut u32_overflow = values.clone();
        let _old = u32_overflow.insert(
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES",
            "4294967296",
        );
        anyhow::ensure!(
            configured(&u32_overflow).is_err(),
            "u32 overflow was accepted"
        );
        let mut u64_overflow = values;
        let _old = u64_overflow.insert(
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES",
            "18446744073709551616",
        );
        anyhow::ensure!(
            configured(&u64_overflow).is_err(),
            "u64 overflow was accepted"
        );
        Ok(())
    }

    #[test]
    fn relational_caps_reject_unfundable_service_and_event_windows() -> Result<()> {
        let valid = configured(&valid_values())?
            .ok_or_else(|| anyhow::anyhow!("valid physical fixture absent"))?;
        let mut invalid = valid;
        invalid.global_sqlite_handles = 15;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "SQLite ceiling below readers accepted"
        );
        invalid = valid;
        invalid.inventory.depth = 2;
        anyhow::ensure!(
            invalid.validate().is_ok(),
            "positive inventory depth was rejected without a census"
        );
        invalid = valid;
        invalid.maintenance.bytes = 1_048_577;
        anyhow::ensure!(invalid.validate().is_err(), "oversized parent accepted");
        invalid = valid;
        invalid.durable_control_file_bytes = 1_048_577;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "oversized durable file accepted"
        );
        invalid = valid;
        invalid.repomap_control_file_bytes = 1_048_577;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "oversized RepoMap Control file accepted"
        );
        invalid = valid;
        invalid.repomap_candidate_file_bytes = 1_048_577;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "oversized RepoMap Candidate file accepted"
        );
        invalid = valid;
        invalid.history.files = 129;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "oversized History parent accepted"
        );
        invalid = valid;
        invalid.inventory.bytes = 60_000;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "concurrent service and event parents exceeded global bytes"
        );
        invalid = valid;
        invalid.inventory.bytes = 65_000;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "concurrent History parent was omitted from global bytes"
        );
        invalid = valid;
        invalid.inventory.bytes = u64::MAX;
        invalid.history.bytes = u64::MAX;
        anyhow::ensure!(
            invalid.validate().is_err(),
            "overflowing concurrent parent sum was accepted"
        );
        let base = SearchdConfig::from_test_state_root(PathBuf::from("/tmp/qi-physical-invalid"));
        anyhow::ensure!(
            base.try_with_legacy_publication_physical_policy(Some(invalid))
                .is_err(),
            "config builder accepted invalid physical limits"
        );
        Ok(())
    }
}
