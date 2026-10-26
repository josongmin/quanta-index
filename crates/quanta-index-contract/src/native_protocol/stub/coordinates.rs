//! Bounded source coordinates shared by Stub producers and consumers.

use super::{NativeStubCoordinateUnit, NativeStubPosition, NativeStubRange};

const MAX_SOURCE_BYTES: usize = 1 << 20;

/// The CR in CRLF is outside the visible line. A trailing LF opens one empty
/// final line, matching the certified Stub producer's source coordinates.
pub(crate) fn line_extent(source: &str, line: u32) -> Option<(usize, usize)> {
    if source.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut current = 0_u32;
    let mut start = 0_usize;
    for (at, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            if current == line {
                let before_newline = at.saturating_sub(1);
                let end = if at > start && source.as_bytes().get(before_newline) == Some(&b'\r') {
                    before_newline
                } else {
                    at
                };
                return Some((start, end));
            }
            current = current.checked_add(1)?;
            start = at.checked_add(1)?;
        }
    }
    (current == line).then_some((start, source.len()))
}

impl NativeStubPosition {
    /// Resolve one visible line/column without accepting a UTF-8 scalar or
    /// UTF-16 surrogate split.
    #[must_use]
    pub fn to_byte(self, source: &str, unit: NativeStubCoordinateUnit) -> Option<usize> {
        let (start, end) = line_extent(source, self.line)?;
        let line = source.get(start..end)?;
        let Ok(column) = usize::try_from(self.column) else {
            return None;
        };
        match unit {
            NativeStubCoordinateUnit::Utf8Bytes => {
                if column > line.len() || !line.is_char_boundary(column) {
                    return None;
                }
                start.checked_add(column)
            }
            NativeStubCoordinateUnit::Utf16Units => {
                let mut units = 0_usize;
                for (at, scalar) in line.char_indices() {
                    if units == column {
                        return start.checked_add(at);
                    }
                    units = units.checked_add(scalar.len_utf16())?;
                    if units > column {
                        return None;
                    }
                }
                (units == column).then_some(end)
            }
        }
    }

    /// Convert a scalar-boundary byte offset to a visible position. CRLF's CR
    /// and LF bytes cannot be selected as positions.
    #[must_use]
    pub fn from_byte(source: &str, byte: usize, unit: NativeStubCoordinateUnit) -> Option<Self> {
        if source.len() > MAX_SOURCE_BYTES || byte > source.len() || !source.is_char_boundary(byte)
        {
            return None;
        }
        let prior = source.get(..byte)?;
        let Ok(line) = u32::try_from(prior.bytes().filter(|value| *value == b'\n').count()) else {
            return None;
        };
        let start = prior.rfind('\n').map_or(Some(0), |at| at.checked_add(1))?;
        let (_, end) = line_extent(source, line)?;
        if byte > end {
            return None;
        }
        let segment = source.get(start..byte)?;
        let column = match unit {
            NativeStubCoordinateUnit::Utf8Bytes => segment.len(),
            NativeStubCoordinateUnit::Utf16Units => {
                segment.chars().try_fold(0_usize, |total, scalar| {
                    total.checked_add(scalar.len_utf16())
                })?
            }
        };
        let Ok(column) = u32::try_from(column) else {
            return None;
        };
        Some(Self { line, column })
    }
}

impl NativeStubRange {
    /// End is exclusive. Reversed or unrepresentable ranges have no byte span.
    #[must_use]
    pub fn to_byte_range(self, source: &str, unit: NativeStubCoordinateUnit) -> Option<(u32, u32)> {
        let start = self.start.to_byte(source, unit)?;
        let end = self.end.to_byte(source, unit)?;
        if start > end {
            return None;
        }
        let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
            return None;
        };
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "a🦀b\r\nz\n";

    #[test]
    fn utf16_columns_reject_surrogate_splits_and_round_trip_scalar_edges() {
        let unit = NativeStubCoordinateUnit::Utf16Units;
        assert_eq!(
            NativeStubPosition { line: 0, column: 1 }.to_byte(SOURCE, unit),
            Some(1)
        );
        assert_eq!(
            NativeStubPosition { line: 0, column: 2 }.to_byte(SOURCE, unit),
            None
        );
        assert_eq!(
            NativeStubPosition { line: 0, column: 3 }.to_byte(SOURCE, unit),
            Some(5)
        );
        assert_eq!(
            NativeStubPosition { line: 0, column: 4 }.to_byte(SOURCE, unit),
            Some(6)
        );
        assert_eq!(
            NativeStubPosition::from_byte(SOURCE, 5, unit),
            Some(NativeStubPosition { line: 0, column: 3 })
        );
        assert_eq!(
            NativeStubPosition::from_byte(SOURCE, 6, unit),
            Some(NativeStubPosition { line: 0, column: 4 })
        );
    }

    #[test]
    fn utf8_crlf_and_trailing_empty_line_match_visible_extents() {
        let unit = NativeStubCoordinateUnit::Utf8Bytes;
        assert_eq!(line_extent(SOURCE, 0), Some((0, 6)));
        assert_eq!(line_extent(SOURCE, 1), Some((8, 9)));
        assert_eq!(line_extent(SOURCE, 2), Some((10, 10)));
        assert_eq!(line_extent(SOURCE, 3), None);
        assert_eq!(
            NativeStubPosition { line: 0, column: 2 }.to_byte(SOURCE, unit),
            None
        );
        assert_eq!(NativeStubPosition::from_byte(SOURCE, 2, unit), None);
        assert_eq!(NativeStubPosition::from_byte(SOURCE, 7, unit), None);
        assert_eq!(
            NativeStubPosition::from_byte(SOURCE, 8, unit),
            Some(NativeStubPosition { line: 1, column: 0 })
        );
        assert_eq!(
            NativeStubPosition { line: 2, column: 0 }.to_byte(SOURCE, unit),
            Some(10)
        );
    }

    #[test]
    fn cross_line_ranges_and_source_cap_fail_closed() {
        let range = NativeStubRange {
            start: NativeStubPosition { line: 0, column: 3 },
            end: NativeStubPosition { line: 1, column: 1 },
        };
        assert_eq!(
            range.to_byte_range(SOURCE, NativeStubCoordinateUnit::Utf16Units),
            Some((5, 9))
        );
        assert_eq!(
            NativeStubRange {
                start: range.end,
                end: range.start,
            }
            .to_byte_range(SOURCE, NativeStubCoordinateUnit::Utf16Units),
            None
        );
        let oversized = "x".repeat(1_048_577);
        assert_eq!(line_extent(&oversized, 0), None);
        assert_eq!(
            NativeStubPosition { line: 0, column: 0 }
                .to_byte(&oversized, NativeStubCoordinateUnit::Utf8Bytes),
            None
        );
        assert_eq!(
            NativeStubPosition::from_byte(&oversized, 0, NativeStubCoordinateUnit::Utf8Bytes),
            None
        );
    }
}
