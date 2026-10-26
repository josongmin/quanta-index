//! Independently verified, per-publication logical merge attribution.
//!
//! Encoded-byte charges describe active index bundles. They are not bytes
//! read or written, unique pool occupancy, process RSS, or reclaimable disk.

use quanta_index_core::CoreError;

/// One File run-posting merge proved by the independent seal.
///
/// File leaf segments are transferred unchanged into the output run. Only
/// the old run-posting bundles leave the target's logical ownership; pins may
/// still retain every underlying object.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileRunMergeFacts {
    pub input_runs: u64,
    pub output_runs: u64,
    pub transferred_segments: u64,
    pub transferred_segment_attributed_bytes: u64,
    pub retired_run_posting_attributed_bytes: u64,
    pub created_run_posting_attributed_bytes: u64,
}

impl FileRunMergeFacts {
    pub(crate) fn merged(
        input_runs: u64,
        transferred_segments: u64,
        transferred_segment_attributed_bytes: u64,
        retired_run_posting_attributed_bytes: u64,
        created_run_posting_attributed_bytes: u64,
    ) -> Result<Self, CoreError> {
        let facts = Self {
            input_runs,
            output_runs: 1,
            transferred_segments,
            transferred_segment_attributed_bytes,
            retired_run_posting_attributed_bytes,
            created_run_posting_attributed_bytes,
        };
        facts.validate()?;
        Ok(facts)
    }

    pub(crate) fn validate(self) -> Result<(), CoreError> {
        if self == Self::default()
            || (self.input_runs >= 2
                && self.output_runs == 1
                && self.transferred_segments >= self.input_runs
                && self.transferred_segment_attributed_bytes > 0
                && self.retired_run_posting_attributed_bytes > 0
                && self.created_run_posting_attributed_bytes > 0)
        {
            Ok(())
        } else {
            Err(CoreError::InvalidContract(
                "File merge facts have an impossible zero or cohort shape".into(),
            ))
        }
    }

    /// Logical input attribution, including unchanged transferred leaves.
    pub fn input_attributed_index_bytes(self) -> Result<u64, CoreError> {
        self.transferred_segment_attributed_bytes
            .checked_add(self.retired_run_posting_attributed_bytes)
            .ok_or_else(|| {
                CoreError::InvalidContract("File merge input attribution overflows".into())
            })
    }

    /// Logical output attribution, including the same transferred leaves.
    pub fn output_attributed_index_bytes(self) -> Result<u64, CoreError> {
        self.transferred_segment_attributed_bytes
            .checked_add(self.created_run_posting_attributed_bytes)
            .ok_or_else(|| {
                CoreError::InvalidContract("File merge output attribution overflows".into())
            })
    }
}

/// Independently attributed Native output segment bundles, when separable
/// from source additions and queued-debt output in the same publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeMergeOutputFacts {
    pub segments: u64,
    pub index_encoded_bytes: u64,
}

/// Independently proved Native clean-run merge input and optional output.
///
/// `output: None` means that output attribution is unavailable for a valid
/// mixed publication. It must not be reported as zero output work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRunMergeFacts {
    pub input_runs: u64,
    pub input_segments: u64,
    pub input_index_encoded_bytes: u64,
    pub output: Option<NativeMergeOutputFacts>,
}

impl Default for NativeRunMergeFacts {
    fn default() -> Self {
        Self {
            input_runs: 0,
            input_segments: 0,
            input_index_encoded_bytes: 0,
            output: Some(NativeMergeOutputFacts::default()),
        }
    }
}

impl NativeRunMergeFacts {
    pub(crate) fn validate(self) -> Result<(), CoreError> {
        if self == Self::default()
            || (self.input_runs >= 2
                && self.input_segments >= self.input_runs
                && self.input_index_encoded_bytes > 0
                && self
                    .output
                    .is_none_or(|output| output.segments > 0 && output.index_encoded_bytes > 0))
        {
            Ok(())
        } else {
            Err(CoreError::InvalidContract(
                "Native merge facts have an impossible zero or cohort shape".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_merge_counts_transferred_leaves_on_both_sides_but_not_as_obsolete() {
        let facts = FileRunMergeFacts::merged(2, 2, 23, 31, 17).expect("fixed cohort");
        assert_eq!(facts.input_attributed_index_bytes().expect("input"), 54);
        assert_eq!(facts.output_attributed_index_bytes().expect("output"), 40);
        assert_eq!(facts.retired_run_posting_attributed_bytes, 31);
        assert_eq!(
            FileRunMergeFacts::default()
                .input_attributed_index_bytes()
                .expect("zero"),
            0
        );
        assert!(FileRunMergeFacts::merged(1, 2, 23, 31, 17).is_err());
    }

    #[test]
    fn native_unavailable_output_is_distinct_from_verified_no_merge() {
        let no_merge = NativeRunMergeFacts::default();
        assert_eq!(no_merge.output, Some(NativeMergeOutputFacts::default()));
        let mixed = NativeRunMergeFacts {
            input_runs: 2,
            input_segments: 3,
            input_index_encoded_bytes: 47,
            output: None,
        };
        mixed.validate().expect("valid unavailable output");
        assert_ne!(mixed, no_merge);
    }
}
