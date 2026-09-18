//! The contract's version stamp.

use core::fmt;

/// Version of the contract in the crate documentation.
///
/// Recorded in every sealed lexical generation and every history text
/// epoch and compared at open. Version 1 was the pre-QI-BB-011 state
/// (Tantivy's default analyzer for the index, whitespace + ASCII folding
/// for the sidecars); it is not readable.
pub const TEXT_NORMALIZER_VERSION: TextNormalizerVersion =
    TextNormalizerVersion { major: 2, minor: 0 };

/// `(major, minor)` of the normalization contract a generation was built with.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextNormalizerVersion {
    pub major: u16,
    pub minor: u16,
}

impl fmt::Display for TextNormalizerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};

    #[test]
    fn the_stamp_renders_as_major_dot_minor() {
        assert_eq!(TEXT_NORMALIZER_VERSION.to_string(), "2.0");
        assert!(TEXT_NORMALIZER_VERSION > TextNormalizerVersion { major: 1, minor: 9 });
    }
}
