//! Borrowed per-source AST projection output. Decoding establishes canonical
//! shape only; a pinned projection and approved parser policy establish trust.

use crate::index::{
    Digest, ObjectRef, OwnerKey, ProducerId, ProjectionId, SchemaId, StableOwnerId,
};
use sha2::{Digest as _, Sha256};

pub const NATIVE_STUB_AST_PROJECTION: ProjectionId = ProjectionId(*b"qidx.ast.proj.v1");
pub const NATIVE_STUB_AST_PRODUCER: ProducerId = ProducerId(*b"smca.ast.prod.v1");
pub const NATIVE_STUB_AST_SCHEMA: SchemaId = SchemaId(*b"qidx.ast.wire.v1");
pub const MAX_NATIVE_STUB_CERTIFICATE_ITEMS: usize = 4096;
pub const MAX_NATIVE_STUB_CERTIFICATE_BYTES: usize = 16 << 20;
const MAGIC: &[u8] = b"QIDXASTCERT1\0";
const MAX_SOURCE_BYTES: u32 = 1 << 20;

#[must_use]
pub fn native_stub_certificate_policy() -> Digest {
    Digest(Sha256::digest(b"quanta-index/native-stub/certificate/v1\0sorted-id256;source-record;grammar-build-policy;node-tree-content;utf8-byte-spans").into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeStubCertificateHeader<'a> {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub owner: OwnerKey,
    pub source_owner: OwnerKey,
    pub source: ObjectRef,
    pub source_bytes: u32,
    pub language: &'a str,
    pub grammar_engine: &'a str,
    pub grammar_version: &'a str,
    pub grammar_hash: Option<[u8; 8]>,
    pub grammar_policy: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeStubCertificateItem<'a> {
    pub stable_id: Digest,
    pub ast_digest: Digest,
    pub start: u32,
    pub end: u32,
    pub label: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeStubCertificateError {
    InvalidHeader,
    InvalidItem,
    InvalidOrder,
    InvalidUtf8,
    InvalidMagic,
    Truncated,
    TrailingBytes,
    LimitExceeded,
    OutputLengthMismatch,
}

impl std::fmt::Display for NativeStubCertificateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid Native AST certificate: {self:?}")
    }
}
impl std::error::Error for NativeStubCertificateError {}

type Result<T> = std::result::Result<T, NativeStubCertificateError>;

fn validate_header(h: &NativeStubCertificateHeader<'_>) -> Result<()> {
    if h.projection != NATIVE_STUB_AST_PROJECTION
        || h.producer != NATIVE_STUB_AST_PRODUCER
        || h.schema != NATIVE_STUB_AST_SCHEMA
        || h.producer_policy != native_stub_certificate_policy()
        || h.owner.producer != h.producer
        || h.owner.stable_id != h.source_owner.stable_id
        || h.source_bytes > MAX_SOURCE_BYTES
        || h.grammar_policy == Digest([0; 32])
    {
        return Err(NativeStubCertificateError::InvalidHeader);
    }
    for (s, cap) in [
        (h.language, 64),
        (h.grammar_engine, 32),
        (h.grammar_version, 128),
    ] {
        if s.is_empty() || s.len() > cap || s.contains('\0') {
            return Err(NativeStubCertificateError::InvalidHeader);
        }
    }
    Ok(())
}

fn validate_item(item: NativeStubCertificateItem<'_>, source_bytes: u32) -> Result<()> {
    if item.start > item.end
        || item.end > source_bytes
        || item.label.len() > 256
        || item.label.is_empty()
        || item.label.contains('\0')
    {
        return Err(NativeStubCertificateError::InvalidItem);
    }
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or(NativeStubCertificateError::LimitExceeded)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(NativeStubCertificateError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_invalid_array| NativeStubCertificateError::Truncated)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn text(&mut self, cap: usize) -> Result<&'a str> {
        let n = usize::from(u16::from_be_bytes(self.array()?));
        if n > cap {
            return Err(NativeStubCertificateError::LimitExceeded);
        }
        std::str::from_utf8(self.take(n)?)
            .map_err(|_invalid_utf8| NativeStubCertificateError::InvalidUtf8)
    }
    fn owner(&mut self) -> Result<OwnerKey> {
        Ok(OwnerKey {
            producer: ProducerId(self.array()?),
            stable_id: StableOwnerId(self.array()?),
        })
    }
    fn item(&mut self) -> Result<NativeStubCertificateItem<'a>> {
        Ok(NativeStubCertificateItem {
            stable_id: Digest(self.array()?),
            ast_digest: Digest(self.array()?),
            start: self.u32()?,
            end: self.u32()?,
            label: self.text(256)?,
        })
    }
}

/// The entire artifact, including ordering and trailing bytes, is checked once.
/// The caller retains its object backing and admission throughout this borrow.
#[derive(Clone, Copy, Debug)]
pub struct NativeStubCertificate<'a> {
    header: NativeStubCertificateHeader<'a>,
    bytes: &'a [u8],
    items_offset: usize,
    count: usize,
}
impl<'a> NativeStubCertificate<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() > MAX_NATIVE_STUB_CERTIFICATE_BYTES {
            return Err(NativeStubCertificateError::LimitExceeded);
        }
        let mut r = Reader { bytes, offset: 0 };
        if r.take(MAGIC.len())? != MAGIC {
            return Err(NativeStubCertificateError::InvalidMagic);
        }
        let header = NativeStubCertificateHeader {
            projection: ProjectionId(r.array()?),
            producer: ProducerId(r.array()?),
            schema: SchemaId(r.array()?),
            producer_policy: Digest(r.array()?),
            owner: r.owner()?,
            source_owner: r.owner()?,
            source: ObjectRef(Digest(r.array()?)),
            source_bytes: r.u32()?,
            language: r.text(64)?,
            grammar_engine: r.text(32)?,
            grammar_version: r.text(128)?,
            grammar_hash: match r.take(1)? {
                [0] => None,
                [1] => Some(r.array()?),
                _ => return Err(NativeStubCertificateError::InvalidHeader),
            },
            grammar_policy: Digest(r.array()?),
        };
        validate_header(&header)?;
        let count = usize::try_from(r.u32()?)
            .map_err(|_unrepresentable| NativeStubCertificateError::LimitExceeded)?;
        if count > MAX_NATIVE_STUB_CERTIFICATE_ITEMS {
            return Err(NativeStubCertificateError::LimitExceeded);
        }
        let items_offset = r.offset;
        let mut previous = None;
        for _ in 0..count {
            let item = r.item()?;
            validate_item(item, header.source_bytes)?;
            if previous.is_some_and(|id| id >= item.stable_id) {
                return Err(NativeStubCertificateError::InvalidOrder);
            }
            previous = Some(item.stable_id);
        }
        if r.offset != bytes.len() {
            return Err(NativeStubCertificateError::TrailingBytes);
        }
        Ok(Self {
            header,
            bytes,
            items_offset,
            count,
        })
    }
    #[must_use]
    pub fn header(self) -> NativeStubCertificateHeader<'a> {
        self.header
    }
    #[must_use]
    pub fn len(self) -> usize {
        self.count
    }
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.count == 0
    }
    #[must_use]
    pub fn iter(self) -> NativeStubCertificateIter<'a> {
        NativeStubCertificateIter {
            reader: Reader {
                bytes: self.bytes,
                offset: self.items_offset,
            },
            remaining: self.count,
        }
    }
}
pub struct NativeStubCertificateIter<'a> {
    reader: Reader<'a>,
    remaining: usize,
}
impl<'a> Iterator for NativeStubCertificateIter<'a> {
    type Item = Result<NativeStubCertificateItem<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining = self.remaining.checked_sub(1)?;
        let item = self.reader.item();
        if item.is_err() {
            self.remaining = 0;
        }
        Some(item)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for NativeStubCertificateIter<'_> {}

pub fn native_stub_certificate_encoded_len(
    header: &NativeStubCertificateHeader<'_>,
    items: &[NativeStubCertificateItem<'_>],
) -> Result<usize> {
    // A counting sink uses the same field writer as the bounded destination.
    write_certificate(header, items, |_| Ok(()))
}

pub fn encode_native_stub_certificate_into(
    header: &NativeStubCertificateHeader<'_>,
    items: &[NativeStubCertificateItem<'_>],
    output: &mut [u8],
) -> Result<()> {
    let expected = native_stub_certificate_encoded_len(header, items)?;
    if output.len() != expected {
        return Err(NativeStubCertificateError::OutputLengthMismatch);
    }
    let mut offset = 0_usize;
    let _written = write_certificate(header, items, |bytes| {
        let end = offset
            .checked_add(bytes.len())
            .ok_or(NativeStubCertificateError::OutputLengthMismatch)?;
        output
            .get_mut(offset..end)
            .ok_or(NativeStubCertificateError::OutputLengthMismatch)?
            .copy_from_slice(bytes);
        offset = end;
        Ok(())
    })?;
    Ok(())
}

fn write_certificate(
    header: &NativeStubCertificateHeader<'_>,
    items: &[NativeStubCertificateItem<'_>],
    mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<usize> {
    validate_header(header)?;
    if items.len() > MAX_NATIVE_STUB_CERTIFICATE_ITEMS {
        return Err(NativeStubCertificateError::LimitExceeded);
    }
    let mut previous = None;
    for &item in items {
        validate_item(item, header.source_bytes)?;
        if previous.is_some_and(|id| id >= item.stable_id) {
            return Err(NativeStubCertificateError::InvalidOrder);
        }
        previous = Some(item.stable_id);
    }
    let mut count = 0usize;
    let mut put = |bytes: &[u8]| {
        count = count
            .checked_add(bytes.len())
            .ok_or(NativeStubCertificateError::LimitExceeded)?;
        if count > MAX_NATIVE_STUB_CERTIFICATE_BYTES {
            return Err(NativeStubCertificateError::LimitExceeded);
        }
        sink(bytes)
    };
    put(MAGIC)?;
    for bytes in [
        &header.projection.0[..],
        &header.producer.0,
        &header.schema.0,
        &header.producer_policy.0,
        &header.owner.producer.0,
        &header.owner.stable_id.0,
        &header.source_owner.producer.0,
        &header.source_owner.stable_id.0,
        &header.source.0.0,
    ] {
        put(bytes)?;
    }
    put(&header.source_bytes.to_be_bytes())?;
    for text in [
        header.language,
        header.grammar_engine,
        header.grammar_version,
    ] {
        put(&u16::try_from(text.len())
            .map_err(|_unrepresentable| NativeStubCertificateError::LimitExceeded)?
            .to_be_bytes())?;
        put(text.as_bytes())?;
    }
    match header.grammar_hash {
        None => put(&[0])?,
        Some(hash) => {
            put(&[1])?;
            put(&hash)?;
        }
    }
    put(&header.grammar_policy.0)?;
    put(&u32::try_from(items.len())
        .map_err(|_unrepresentable| NativeStubCertificateError::LimitExceeded)?
        .to_be_bytes())?;
    for item in items {
        put(&item.stable_id.0)?;
        put(&item.ast_digest.0)?;
        put(&item.start.to_be_bytes())?;
        put(&item.end.to_be_bytes())?;
        put(&u16::try_from(item.label.len())
            .map_err(|_unrepresentable| NativeStubCertificateError::LimitExceeded)?
            .to_be_bytes())?;
        put(item.label.as_bytes())?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("condition failed: {}", stringify!($condition)).into());
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let (left, right) = (&$left, &$right);
            if left != right {
                return Err(format!("values differ: {left:?} != {right:?}").into());
            }
        }};
    }

    fn header() -> NativeStubCertificateHeader<'static> {
        NativeStubCertificateHeader {
            projection: NATIVE_STUB_AST_PROJECTION,
            producer: NATIVE_STUB_AST_PRODUCER,
            schema: NATIVE_STUB_AST_SCHEMA,
            producer_policy: native_stub_certificate_policy(),
            owner: OwnerKey {
                producer: NATIVE_STUB_AST_PRODUCER,
                stable_id: StableOwnerId([3; 16]),
            },
            source_owner: OwnerKey {
                producer: ProducerId([4; 16]),
                stable_id: StableOwnerId([3; 16]),
            },
            source: ObjectRef(Digest([5; 32])),
            source_bytes: 9,
            language: "java",
            grammar_engine: "tree-sitter",
            grammar_version: "0.23.5",
            grammar_hash: None,
            grammar_policy: Digest([6; 32]),
        }
    }
    fn item() -> NativeStubCertificateItem<'static> {
        NativeStubCertificateItem {
            stable_id: Digest([7; 32]),
            ast_digest: Digest([8; 32]),
            start: 0,
            end: 9,
            label: "Example",
        }
    }
    fn wire(items: &[NativeStubCertificateItem<'_>]) -> Result<Vec<u8>> {
        let mut output = vec![0; native_stub_certificate_encoded_len(&header(), items)?];
        encode_native_stub_certificate_into(&header(), items, &mut output)?;
        Ok(output)
    }
    #[test]
    fn certificate_wire_matches_independent_fixed_golden() -> TestResult {
        // Independently specified big-endian field stream, not a second encoder
        // invocation or a golden regenerated by the implementation under test.
        const SHA256: [u8; 32] = [
            33, 230, 184, 35, 49, 249, 245, 147, 33, 240, 21, 21, 192, 221, 217, 16, 41, 18, 42,
            214, 25, 157, 184, 131, 115, 194, 79, 3, 26, 54, 14, 15,
        ];
        let output = wire(&[item()])?;
        check_eq!(output.len(), 338);
        check_eq!(<[u8; 32]>::from(Sha256::digest(&output)), SHA256);
        let parsed = NativeStubCertificate::parse(&output)?;
        check_eq!(parsed.header(), header());
        check_eq!(parsed.iter().collect::<Result<Vec<_>>>()?, [item()]);
        Ok(())
    }
    #[test]
    fn empty_certificate_is_explicit_and_all_truncations_refuse() -> TestResult {
        let output = wire(&[])?;
        check!(NativeStubCertificate::parse(&output)?.is_empty());
        let full = wire(&[item()])?;
        for end in 0..full.len() {
            check!(
                NativeStubCertificate::parse(
                    full.get(..end)
                        .ok_or(NativeStubCertificateError::Truncated)?
                )
                .is_err()
            );
        }
        let mut trailing = full;
        trailing.push(0);
        check!(matches!(
            NativeStubCertificate::parse(&trailing),
            Err(NativeStubCertificateError::TrailingBytes)
        ));
        Ok(())
    }
    #[test]
    fn duplicate_identity_and_foreign_producer_refuse_before_writing() -> TestResult {
        let items = [item(), item()];
        check_eq!(
            native_stub_certificate_encoded_len(&header(), &items),
            Err(NativeStubCertificateError::InvalidOrder)
        );
        let mut foreign = header();
        foreign.owner.producer = foreign.source_owner.producer;
        let mut untouched = [0x55; 338];
        check_eq!(
            encode_native_stub_certificate_into(&foreign, &[item()], &mut untouched),
            Err(NativeStubCertificateError::InvalidHeader)
        );
        check!(untouched.iter().all(|&byte| byte == 0x55));
        Ok(())
    }
    #[test]
    fn parser_checks_grammar_tag_policy_order_utf8_and_span() -> TestResult {
        let base = wire(&[item()])?;
        let mut missing_policy = header();
        missing_policy.grammar_policy = Digest([0; 32]);
        check_eq!(
            native_stub_certificate_encoded_len(&missing_policy, &[item()]),
            Err(NativeStubCertificateError::InvalidHeader)
        );
        let mut bad = base;
        *bad.last_mut()
            .ok_or(NativeStubCertificateError::Truncated)? = 0xff;
        check!(matches!(
            NativeStubCertificate::parse(&bad),
            Err(NativeStubCertificateError::InvalidUtf8)
        ));
        let mut bad_span = item();
        bad_span.end = 10;
        check_eq!(
            native_stub_certificate_encoded_len(&header(), &[bad_span]),
            Err(NativeStubCertificateError::InvalidItem)
        );
        let mut earlier = item();
        earlier.stable_id = Digest([1; 32]);
        check_eq!(
            native_stub_certificate_encoded_len(&header(), &[item(), earlier]),
            Err(NativeStubCertificateError::InvalidOrder)
        );
        let mut hash_header = header();
        hash_header.grammar_hash = Some([9; 8]);
        let mut hashed = vec![0; native_stub_certificate_encoded_len(&hash_header, &[item()])?];
        encode_native_stub_certificate_into(&hash_header, &[item()], &mut hashed)?;
        check_eq!(
            NativeStubCertificate::parse(&hashed)?.header().grammar_hash,
            Some([9; 8])
        );
        Ok(())
    }
}
