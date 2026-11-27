//! The one canonical relative Source path rule shared by every Native surface.

use super::MAX_SOURCE_PATH_BYTES;

/// Whether `path` is a canonical relative Source path accepted by Native
/// publication: nonempty, at most [`MAX_SOURCE_PATH_BYTES`] bytes, no leading
/// `/`, no NUL, and no empty, `.` or `..` segment.
#[must_use]
pub fn is_canonical_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_source_path_rule_table() {
        let longest = "a".repeat(MAX_SOURCE_PATH_BYTES);
        let too_long = "a".repeat(
            MAX_SOURCE_PATH_BYTES
                .checked_add(1)
                .expect("bounded length"),
        );
        for (path, expected) in [
            ("a.rs", true),
            ("src/lib.rs", true),
            ("src/검색.rs", true),
            ("a/.hidden/..b", true),
            (longest.as_str(), true),
            ("", false),
            (too_long.as_str(), false),
            ("/src/lib.rs", false),
            ("src//lib.rs", false),
            ("src/lib.rs/", false),
            ("./src/lib.rs", false),
            ("src/./lib.rs", false),
            ("src/../lib.rs", false),
            ("..", false),
            ("src/li\0b.rs", false),
        ] {
            assert_eq!(is_canonical_source_path(path), expected, "{path:?}");
        }
    }
}
