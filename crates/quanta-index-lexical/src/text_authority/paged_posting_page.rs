//! Canonical multi-posting pages. Many route keys share one authenticated blob.

use quanta_index_core::CoreError;

use crate::text_authority::paged_segment::BuiltPosting;

const MAGIC: &[u8; 8] = b"QNPOST01";
pub(crate) const TARGET_BYTES: usize = 32 * 1024;
pub(crate) const HEADER_BYTES: usize = 18;
pub(crate) const ROW_HEADER_BYTES: usize = 7;

#[derive(Clone, Debug)]
pub(crate) struct PostingRow<'a> {
    pub(crate) surface: u8,
    pub(crate) key: &'a [u8],
    pub(crate) payload: &'a [u8],
}

#[derive(Clone, Debug)]
pub(crate) struct PostingPage<'a> {
    pub(crate) rows: Vec<PostingRow<'a>>,
}

impl<'a> PostingPage<'a> {
    pub(crate) fn encoded_len_for(row: &BuiltPosting) -> Result<usize, CoreError> {
        ROW_HEADER_BYTES
            .checked_add(row.key.len())
            .and_then(|bytes| bytes.checked_add(row.bytes.len()))
            .ok_or_else(|| corrupt("posting page row length overflows"))
    }

    pub(crate) fn encode(segment_key: u64, rows: &[&BuiltPosting]) -> Result<Vec<u8>, CoreError> {
        if segment_key == 0 || rows.is_empty() || rows.len() > usize::from(u16::MAX) {
            return Err(corrupt("posting page segment or row count differs"));
        }
        let mut size = HEADER_BYTES;
        let mut prior: Option<(u8, &[u8])> = None;
        for row in rows {
            if u16::try_from(row.key.len()).is_err()
                || u32::try_from(row.bytes.len()).is_err()
                || prior.is_some_and(|(surface, key)| {
                    (surface, key) >= (row.surface, row.key.as_slice())
                })
            {
                return Err(corrupt("posting page rows are not canonical"));
            }
            size = size
                .checked_add(Self::encoded_len_for(row)?)
                .ok_or_else(|| corrupt("posting page length overflows"))?;
            prior = Some((row.surface, &row.key));
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native posting page: {error}"))
        })?;
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&segment_key.to_be_bytes());
        bytes.extend_from_slice(
            &u16::try_from(rows.len())
                .map_err(|error| corrupt(&format!("page count width: {error}")))?
                .to_be_bytes(),
        );
        for row in rows {
            bytes.push(row.surface);
            bytes.extend_from_slice(
                &u16::try_from(row.key.len())
                    .map_err(|error| corrupt(&format!("page key width: {error}")))?
                    .to_be_bytes(),
            );
            bytes.extend_from_slice(
                &u32::try_from(row.bytes.len())
                    .map_err(|error| corrupt(&format!("page payload width: {error}")))?
                    .to_be_bytes(),
            );
            bytes.extend_from_slice(&row.key);
            bytes.extend_from_slice(&row.bytes);
        }
        if bytes.len() != size {
            return Err(corrupt("posting page encoded length differs"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &'a [u8], segment_key: u64) -> Result<Self, CoreError> {
        if bytes.len() < HEADER_BYTES || bytes.get(..8) != Some(MAGIC.as_slice()) {
            return Err(corrupt("posting page magic or length differs"));
        }
        let actual_segment = u64::from_be_bytes(read_fixed::<8>(bytes, 8, "page segment")?);
        if actual_segment != segment_key {
            return Err(corrupt("posting page segment differs"));
        }
        let count = usize::from(u16::from_be_bytes(read_fixed::<2>(
            bytes,
            16,
            "page count",
        )?));
        if count == 0 {
            return Err(corrupt("posting page is empty"));
        }
        if count
            > bytes
                .len()
                .saturating_sub(HEADER_BYTES)
                .div_euclid(ROW_HEADER_BYTES)
        {
            return Err(corrupt("posting page row count exceeds bytes"));
        }
        let mut offset = HEADER_BYTES;
        let mut rows = Vec::new();
        rows.try_reserve(count).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve decoded Native posting rows: {error}"
            ))
        })?;
        for _ in 0..count {
            let header = bytes
                .get(offset..offset.saturating_add(ROW_HEADER_BYTES))
                .ok_or_else(|| corrupt("posting page row header truncated"))?;
            let surface = *header
                .first()
                .ok_or_else(|| corrupt("posting surface missing"))?;
            let key_len = usize::from(u16::from_be_bytes(read_fixed::<2>(
                header,
                1,
                "key length",
            )?));
            let payload_len = usize::try_from(u32::from_be_bytes(read_fixed::<4>(
                header,
                3,
                "payload length",
            )?))
            .map_err(|error| corrupt(&format!("posting page payload width: {error}")))?;
            offset = offset.saturating_add(ROW_HEADER_BYTES);
            let key = bytes
                .get(offset..offset.saturating_add(key_len))
                .ok_or_else(|| corrupt("posting page key truncated"))?;
            offset = offset.saturating_add(key_len);
            let payload = bytes
                .get(offset..offset.saturating_add(payload_len))
                .ok_or_else(|| corrupt("posting page payload truncated"))?;
            offset = offset.saturating_add(payload_len);
            if rows
                .last()
                .is_some_and(|prior: &PostingRow<'a>| (prior.surface, prior.key) >= (surface, key))
            {
                return Err(corrupt("posting page rows repeat or are not sorted"));
            }
            rows.push(PostingRow {
                surface,
                key,
                payload,
            });
        }
        if offset != bytes.len() {
            return Err(corrupt("posting page has trailing bytes"));
        }
        Ok(Self { rows })
    }

    pub(crate) fn posting(&self, surface: u8, key: &[u8]) -> Option<&[u8]> {
        self.rows
            .binary_search_by(|row| (row.surface, row.key).cmp(&(surface, key)))
            .map_or(None, |index| self.rows.get(index).map(|row| row.payload))
    }
}

pub(crate) fn route_posting_key(route: &[u8], segment_key: u64) -> Result<(u8, &[u8]), CoreError> {
    let surface = *route.first().ok_or_else(|| corrupt("route is empty"))?;
    let tail = route
        .get(route.len().saturating_sub(8)..)
        .ok_or_else(|| corrupt("route segment suffix missing"))?;
    if tail != segment_key.to_be_bytes() {
        return Err(corrupt("route segment suffix differs"));
    }
    match surface {
        1 | 2 if route.len() == 12 => Ok((
            surface,
            route
                .get(1..4)
                .ok_or_else(|| corrupt("gram route key missing"))?,
        )),
        3 | 4 if route.len() >= 11 => {
            let length = usize::from(u16::from_be_bytes(read_fixed::<2>(
                route,
                1,
                "term route length",
            )?));
            if route.len() != length.saturating_add(11) {
                return Err(corrupt("term route length differs"));
            }
            let end = 3_usize
                .checked_add(length)
                .ok_or_else(|| corrupt("term route length overflows"))?;
            Ok((
                surface,
                route
                    .get(3..end)
                    .ok_or_else(|| corrupt("term route key missing"))?,
            ))
        }
        _ => Err(corrupt("route surface or length differs")),
    }
}

fn read_fixed<const N: usize>(
    bytes: &[u8],
    offset: usize,
    field: &str,
) -> Result<[u8; N], CoreError> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| corrupt("field offset overflows"))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| corrupt(&format!("{field} truncated")))?
        .try_into()
        .map_err(|error| corrupt(&format!("{field} width: {error}")))
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!("lexical: Native posting page: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{PostingPage, route_posting_key};
    use crate::text_authority::paged_segment::BuiltPosting;
    use crate::text_authority::paged_wire::{gram_prefix, route_key, term_prefix};

    #[test]
    fn distinct_rows_share_one_page_and_selected_key_is_exact()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = BuiltPosting {
            surface: 1,
            key: b"abc".to_vec(),
            bytes: vec![1, 2],
        };
        let second = BuiltPosting {
            surface: 1,
            key: b"def".to_vec(),
            bytes: vec![3, 4],
        };
        let bytes = PostingPage::encode(7, &[&first, &second])?;
        let page = PostingPage::decode(&bytes, 7)?;
        if page.posting(1, b"abc") != Some(&[1, 2][..])
            || page.posting(1, b"def") != Some(&[3, 4][..])
            || page.posting(1, b"xyz").is_some()
            || PostingPage::decode(&bytes, 8).is_ok()
        {
            return Err("posting page selection or segment binding differs".into());
        }
        Ok(())
    }

    #[test]
    fn malformed_count_and_duplicate_rows_refuse() -> Result<(), Box<dyn std::error::Error>> {
        let row = BuiltPosting {
            surface: 1,
            key: b"abc".to_vec(),
            bytes: vec![9],
        };
        if PostingPage::encode(3, &[&row, &row]).is_ok() {
            return Err("duplicate posting rows were accepted".into());
        }
        let mut bytes = PostingPage::encode(3, &[&row])?;
        *bytes.get_mut(16).ok_or("missing posting count byte 0")? = u8::MAX;
        *bytes.get_mut(17).ok_or("missing posting count byte 1")? = u8::MAX;
        if PostingPage::decode(&bytes, 3).is_ok() {
            return Err("forged posting row count was accepted".into());
        }
        Ok(())
    }

    #[test]
    fn route_identity_accepts_gram_and_term_only_at_exact_segment()
    -> Result<(), Box<dyn std::error::Error>> {
        let gram = route_key(&gram_prefix(1, *b"abc"), 5);
        if route_posting_key(&gram, 5)? != (1, &b"abc"[..]) || route_posting_key(&gram, 6).is_ok() {
            return Err("gram route identity differs".into());
        }
        let term = route_key(&term_prefix(3, "alpha")?, 5);
        if route_posting_key(&term, 5)? != (3, &b"alpha"[..]) {
            return Err("term route identity differs".into());
        }
        Ok(())
    }
}
