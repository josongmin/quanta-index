//! Bounded, versioned wire for a certified projection's semantic members.
//!
//! Parsing authenticates neither the producer nor the selected projection
//! object. The adapter must compare this header with the selected certificate
//! and contribution before it uses any member as embedding input.

use crate::index::{
    Digest, ObjectRef, OwnerKey, ProducerId, ProjectionId, ProjectionOwnerContribution, SchemaId,
    StableOwnerId,
};

/// A complete manifest fits inside one native projection output object.
const MAX_NATIVE_SEMANTIC_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
/// A manifest's member count is bounded before any member is parsed.
const MAX_NATIVE_SEMANTIC_MANIFEST_MEMBERS: u32 = 4_096;

const MAGIC: &[u8; 14] = b"QIDXSEMCHUNK1\0";

/// Identity and dependency stamp of the certified owner contribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSemanticManifestHeader {
    pub owner: OwnerKey,
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub schema: SchemaId,
    pub source: Option<ObjectRef>,
    pub dependency_stamp: Digest,
}

impl NativeSemanticManifestHeader {
    /// Use the canonical contribution hash, including its original domain.
    /// The synthetic contribution only supplies fields consumed by that hash;
    /// it does not attest publication or issue a certificate.
    #[must_use]
    pub fn materialization_context(self) -> Digest {
        ProjectionOwnerContribution {
            projection: self.projection,
            producer: self.producer,
            owner: self.owner,
            source_owner: self.source.map(|_| self.owner),
            source: self.source,
            dependency_stamp: self.dependency_stamp,
            output: None,
        }
        .materialization_context()
    }
}

/// One stable producer identity and the exact rendered UTF-8 embedding input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSemanticManifestMember<'a> {
    pub id: Digest,
    pub text: &'a str,
}

/// Caller policy may tighten either global cap, but may not expand it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSemanticManifestLimits {
    pub max_members: u32,
    pub max_member_utf8_bytes: u32,
}

impl NativeSemanticManifestLimits {
    pub const MAXIMUM: Self = Self {
        max_members: MAX_NATIVE_SEMANTIC_MANIFEST_MEMBERS,
        max_member_utf8_bytes: 16_777_216,
    };

    fn validate(self) -> Result<(), NativeSemanticManifestError> {
        if self.max_members == 0
            || self.max_members > MAX_NATIVE_SEMANTIC_MANIFEST_MEMBERS
            || usize::try_from(self.max_member_utf8_bytes)
                .map_or(true, |length| length > MAX_NATIVE_SEMANTIC_MANIFEST_BYTES)
        {
            return Err(NativeSemanticManifestError::InvalidLimits);
        }
        Ok(())
    }
}

/// Closed wire failures; malformed input is never interpreted as an empty owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSemanticManifestError {
    InvalidLimits,
    PayloadTooLarge,
    InvalidMagic,
    Truncated,
    TrailingBytes,
    InvalidSourceTag,
    InvalidMemberCount,
    MemberTooLarge,
    InvalidUtf8,
    LengthOverflow,
    OutputLengthMismatch,
}

impl std::fmt::Display for NativeSemanticManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLimits => "semantic manifest limits exceed the native wire caps",
            Self::PayloadTooLarge => "semantic manifest exceeds the native payload cap",
            Self::InvalidMagic => "semantic manifest has an unknown wire version",
            Self::Truncated => "semantic manifest is truncated",
            Self::TrailingBytes => "semantic manifest has trailing bytes",
            Self::InvalidSourceTag => "semantic manifest has an invalid source presence tag",
            Self::InvalidMemberCount => "semantic manifest requires a bounded nonempty member set",
            Self::MemberTooLarge => "semantic manifest member exceeds the UTF-8 byte cap",
            Self::InvalidUtf8 => "semantic manifest member is not UTF-8",
            Self::LengthOverflow => "semantic manifest encoded length overflows",
            Self::OutputLengthMismatch => "semantic manifest destination has the wrong length",
        })
    }
}

impl std::error::Error for NativeSemanticManifestError {}

/// Borrowed complete manifest. `parse` checks every member and trailing byte
/// before this value is returned; iteration does not allocate.
#[derive(Clone, Copy, Debug)]
pub struct NativeSemanticChunkManifest<'a> {
    header: NativeSemanticManifestHeader,
    payload: &'a [u8],
    members_offset: usize,
    count: usize,
}

impl<'a> NativeSemanticChunkManifest<'a> {
    pub fn parse(
        payload: &'a [u8],
        limits: NativeSemanticManifestLimits,
    ) -> Result<Self, NativeSemanticManifestError> {
        limits.validate()?;
        if payload.len() > MAX_NATIVE_SEMANTIC_MANIFEST_BYTES {
            return Err(NativeSemanticManifestError::PayloadTooLarge);
        }
        let mut reader = Reader::new(payload);
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(NativeSemanticManifestError::InvalidMagic);
        }
        let owner = OwnerKey {
            producer: ProducerId(reader.array()?),
            stable_id: StableOwnerId(reader.array()?),
        };
        let projection = ProjectionId(reader.array()?);
        let producer = ProducerId(reader.array()?);
        let schema = SchemaId(reader.array()?);
        let source = match reader.byte()? {
            0 => None,
            1 => Some(ObjectRef(Digest(reader.array()?))),
            _ => return Err(NativeSemanticManifestError::InvalidSourceTag),
        };
        let dependency_stamp = Digest(reader.array()?);
        let count = reader.u32()?;
        validate_count(count, limits)?;
        let members_offset = reader.offset;
        for _ in 0..count {
            let _validated_member = read_member(&mut reader, limits)?;
        }
        if reader.offset != payload.len() {
            return Err(NativeSemanticManifestError::TrailingBytes);
        }
        Ok(Self {
            header: NativeSemanticManifestHeader {
                owner,
                projection,
                producer,
                schema,
                source,
                dependency_stamp,
            },
            payload,
            members_offset,
            count: usize::try_from(count)
                .map_err(|_error| NativeSemanticManifestError::LengthOverflow)?,
        })
    }

    #[must_use]
    pub fn header(self) -> NativeSemanticManifestHeader {
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

    /// Each item remains fallible so an impossible internal offset error does
    /// not become a panic if the parser implementation changes.
    #[must_use]
    pub fn iter(self) -> NativeSemanticManifestIter<'a> {
        NativeSemanticManifestIter {
            reader: Reader {
                payload: self.payload,
                offset: self.members_offset,
            },
            remaining: self.count,
            limits: NativeSemanticManifestLimits::MAXIMUM,
        }
    }
}

pub struct NativeSemanticManifestIter<'a> {
    reader: Reader<'a>,
    remaining: usize,
    limits: NativeSemanticManifestLimits,
}

impl<'a> Iterator for NativeSemanticManifestIter<'a> {
    type Item = Result<NativeSemanticManifestMember<'a>, NativeSemanticManifestError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining = self.remaining.saturating_sub(1);
        let member = read_member(&mut self.reader, self.limits);
        if member.is_err() {
            self.remaining = 0;
        }
        Some(member)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining;
        (remaining, Some(remaining))
    }
}

impl std::iter::ExactSizeIterator for NativeSemanticManifestIter<'_> {}

/// Compute the exact destination length before allocating or staging output.
pub fn native_semantic_manifest_encoded_len(
    header: NativeSemanticManifestHeader,
    members: &[NativeSemanticManifestMember<'_>],
    limits: NativeSemanticManifestLimits,
) -> Result<usize, NativeSemanticManifestError> {
    limits.validate()?;
    let count = u32::try_from(members.len())
        .map_err(|_error| NativeSemanticManifestError::InvalidMemberCount)?;
    validate_count(count, limits)?;
    let header_len = MAGIC
        .len()
        .checked_add(117) // owner, projection, producer, schema, source tag, stamp, count
        .and_then(|length| length.checked_add(if header.source.is_some() { 32 } else { 0 }))
        .ok_or(NativeSemanticManifestError::LengthOverflow)?;
    let mut length = header_len;
    for member in members {
        let bytes = member.text.len();
        validate_member_len(bytes, limits)?;
        length = length
            .checked_add(36) // member identity and byte length
            .and_then(|length| length.checked_add(bytes))
            .ok_or(NativeSemanticManifestError::LengthOverflow)?;
        if length > MAX_NATIVE_SEMANTIC_MANIFEST_BYTES {
            return Err(NativeSemanticManifestError::PayloadTooLarge);
        }
    }
    Ok(length)
}

/// Encode into the caller's exact pre-admitted buffer. Validation completes
/// before writing the first byte; the codec never allocates a second payload.
pub fn encode_native_semantic_manifest_into(
    header: NativeSemanticManifestHeader,
    members: &[NativeSemanticManifestMember<'_>],
    limits: NativeSemanticManifestLimits,
    output: &mut [u8],
) -> Result<(), NativeSemanticManifestError> {
    let length = native_semantic_manifest_encoded_len(header, members, limits)?;
    if output.len() != length {
        return Err(NativeSemanticManifestError::OutputLengthMismatch);
    }
    let mut writer = Writer::new(output);
    writer.put(MAGIC)?;
    writer.put(&header.owner.producer.0)?;
    writer.put(&header.owner.stable_id.0)?;
    writer.put(&header.projection.0)?;
    writer.put(&header.producer.0)?;
    writer.put(&header.schema.0)?;
    match header.source {
        None => writer.put(&[0])?,
        Some(source) => {
            writer.put(&[1])?;
            writer.put(&source.0.0)?;
        }
    }
    writer.put(&header.dependency_stamp.0)?;
    writer.put(
        &u32::try_from(members.len())
            .map_err(|_error| NativeSemanticManifestError::InvalidMemberCount)?
            .to_be_bytes(),
    )?;
    for member in members {
        writer.put(&member.id.0)?;
        writer.put(
            &u32::try_from(member.text.len())
                .map_err(|_error| NativeSemanticManifestError::MemberTooLarge)?
                .to_be_bytes(),
        )?;
        writer.put(member.text.as_bytes())?;
    }
    if writer.offset != length {
        return Err(NativeSemanticManifestError::OutputLengthMismatch);
    }
    Ok(())
}

fn validate_count(
    count: u32,
    limits: NativeSemanticManifestLimits,
) -> Result<(), NativeSemanticManifestError> {
    if count == 0 || count > limits.max_members {
        return Err(NativeSemanticManifestError::InvalidMemberCount);
    }
    Ok(())
}

fn validate_member_len(
    length: usize,
    limits: NativeSemanticManifestLimits,
) -> Result<(), NativeSemanticManifestError> {
    let max_length = usize::try_from(limits.max_member_utf8_bytes)
        .map_err(|_error| NativeSemanticManifestError::LengthOverflow)?;
    if length > max_length {
        return Err(NativeSemanticManifestError::MemberTooLarge);
    }
    Ok(())
}

fn read_member<'a>(
    reader: &mut Reader<'a>,
    limits: NativeSemanticManifestLimits,
) -> Result<NativeSemanticManifestMember<'a>, NativeSemanticManifestError> {
    let id = Digest(reader.array()?);
    let length = usize::try_from(reader.u32()?)
        .map_err(|_error| NativeSemanticManifestError::LengthOverflow)?;
    validate_member_len(length, limits)?;
    let bytes = reader.take(length)?;
    let text =
        std::str::from_utf8(bytes).map_err(|_error| NativeSemanticManifestError::InvalidUtf8)?;
    Ok(NativeSemanticManifestMember { id, text })
}

#[derive(Clone, Copy, Debug)]
struct Reader<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], NativeSemanticManifestError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(NativeSemanticManifestError::LengthOverflow)?;
        let bytes = self
            .payload
            .get(self.offset..end)
            .ok_or(NativeSemanticManifestError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NativeSemanticManifestError> {
        self.take(N)?
            .try_into()
            .map_err(|_error| NativeSemanticManifestError::Truncated)
    }

    fn byte(&mut self) -> Result<u8, NativeSemanticManifestError> {
        self.take(1)?
            .first()
            .copied()
            .ok_or(NativeSemanticManifestError::Truncated)
    }

    fn u32(&mut self) -> Result<u32, NativeSemanticManifestError> {
        Ok(u32::from_be_bytes(self.array()?))
    }
}

struct Writer<'a> {
    output: &'a mut [u8],
    offset: usize,
}

impl<'a> Writer<'a> {
    fn new(output: &'a mut [u8]) -> Self {
        Self { output, offset: 0 }
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), NativeSemanticManifestError> {
        let end = self
            .offset
            .checked_add(bytes.len())
            .ok_or(NativeSemanticManifestError::LengthOverflow)?;
        let destination = self
            .output
            .get_mut(self.offset..end)
            .ok_or(NativeSemanticManifestError::OutputLengthMismatch)?;
        destination.copy_from_slice(bytes);
        self.offset = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("condition failed: {}", stringify!($condition)).into());
            }
        };
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!(
                    "values differ: {} and {}",
                    stringify!($left),
                    stringify!($right)
                )
                .into());
            }
        }};
    }

    fn header(source: Option<ObjectRef>) -> NativeSemanticManifestHeader {
        NativeSemanticManifestHeader {
            owner: OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([2; 16]),
            },
            projection: ProjectionId([3; 16]),
            producer: ProducerId([4; 16]),
            schema: SchemaId([5; 16]),
            source,
            dependency_stamp: Digest([7; 32]),
        }
    }

    // This fixture is specified as literal wire fields; the encoder is not
    // used to create the expected bytes or expected text.
    fn fixed_gold(source: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"QIDXSEMCHUNK1\0");
        bytes.extend_from_slice(&[1; 16]);
        bytes.extend_from_slice(&[2; 16]);
        bytes.extend_from_slice(&[3; 16]);
        bytes.extend_from_slice(&[4; 16]);
        bytes.extend_from_slice(&[5; 16]);
        bytes.push(u8::from(source));
        if source {
            bytes.extend_from_slice(&[6; 32]);
        }
        bytes.extend_from_slice(&[7; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 2]);
        bytes.extend_from_slice(&[8; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 2]);
        bytes.extend_from_slice(b"\xce\xb1");
        bytes.extend_from_slice(&[9; 32]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn fixed_wire_parses_and_encodes_with_and_without_source()
    -> Result<(), Box<dyn std::error::Error>> {
        for source in [false, true] {
            let source_ref = source.then_some(ObjectRef(Digest([6; 32])));
            let gold = fixed_gold(source);
            let parsed =
                NativeSemanticChunkManifest::parse(&gold, NativeSemanticManifestLimits::MAXIMUM)?;
            check_eq!(parsed.header(), header(source_ref));
            check_eq!(parsed.len(), 2);
            let members = parsed.iter().collect::<Result<Vec<_>, _>>()?;
            check_eq!(
                members,
                [
                    NativeSemanticManifestMember {
                        id: Digest([8; 32]),
                        text: "α"
                    },
                    NativeSemanticManifestMember {
                        id: Digest([9; 32]),
                        text: ""
                    },
                ]
            );
            let mut encoded = vec![
                0xff;
                native_semantic_manifest_encoded_len(
                    header(source_ref),
                    &members,
                    NativeSemanticManifestLimits::MAXIMUM
                )?
            ];
            encode_native_semantic_manifest_into(
                header(source_ref),
                &members,
                NativeSemanticManifestLimits::MAXIMUM,
                &mut encoded,
            )?;
            check_eq!(encoded, gold);
        }
        Ok(())
    }

    #[test]
    fn malformed_manifest_is_rejected_before_exposing_any_member()
    -> Result<(), Box<dyn std::error::Error>> {
        let gold = fixed_gold(false);
        for cut in 0..gold.len() {
            check!(
                NativeSemanticChunkManifest::parse(
                    gold.get(..cut)
                        .ok_or(NativeSemanticManifestError::Truncated)?,
                    NativeSemanticManifestLimits::MAXIMUM
                )
                .is_err()
            );
        }
        let mut bad = gold.clone();
        *bad.get_mut(0)
            .ok_or(NativeSemanticManifestError::Truncated)? = b'X';
        check_eq!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM).err(),
            Some(NativeSemanticManifestError::InvalidMagic)
        );
        let mut bad = gold.clone();
        // Source presence tag follows magic and five 16-byte identities.
        *bad.get_mut(94)
            .ok_or(NativeSemanticManifestError::Truncated)? = 2;
        check_eq!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM).err(),
            Some(NativeSemanticManifestError::InvalidSourceTag)
        );
        let mut bad = gold.clone();
        // Third byte of the second member length changes zero to 65,280.
        *bad.get_mut(203)
            .ok_or(NativeSemanticManifestError::Truncated)? = 0xff;
        check!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM)
                .is_err()
        );
        let mut bad = gold.clone();
        *bad.get_mut(167)
            .ok_or(NativeSemanticManifestError::Truncated)? = 0xff;
        check_eq!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM).err(),
            Some(NativeSemanticManifestError::InvalidUtf8)
        );
        let mut bad = gold.clone();
        bad.push(0);
        check_eq!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM).err(),
            Some(NativeSemanticManifestError::TrailingBytes)
        );
        let mut bad = gold;
        bad.get_mut(127..131)
            .ok_or(NativeSemanticManifestError::Truncated)?
            .copy_from_slice(&0_u32.to_be_bytes());
        check_eq!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM).err(),
            Some(NativeSemanticManifestError::InvalidMemberCount)
        );
        bad.get_mut(127..131)
            .ok_or(NativeSemanticManifestError::Truncated)?
            .copy_from_slice(&4_097_u32.to_be_bytes());
        check_eq!(
            NativeSemanticChunkManifest::parse(&bad, NativeSemanticManifestLimits::MAXIMUM).err(),
            Some(NativeSemanticManifestError::InvalidMemberCount)
        );
        Ok(())
    }

    #[test]
    fn limits_and_exact_destination_apply_before_writing() -> Result<(), Box<dyn std::error::Error>>
    {
        let members = [NativeSemanticManifestMember {
            id: Digest([8; 32]),
            text: "α",
        }];
        let limits = NativeSemanticManifestLimits {
            max_members: 1,
            max_member_utf8_bytes: 1,
        };
        check_eq!(
            native_semantic_manifest_encoded_len(header(None), &members, limits),
            Err(NativeSemanticManifestError::MemberTooLarge)
        );
        let limits = NativeSemanticManifestLimits::MAXIMUM;
        let length = native_semantic_manifest_encoded_len(header(None), &members, limits)?;
        let mut destination = vec![0xaa; length.saturating_sub(1)];
        check_eq!(
            encode_native_semantic_manifest_into(header(None), &members, limits, &mut destination),
            Err(NativeSemanticManifestError::OutputLengthMismatch)
        );
        check!(destination.iter().all(|byte| *byte == 0xaa));
        check_eq!(
            native_semantic_manifest_encoded_len(header(None), &[], limits),
            Err(NativeSemanticManifestError::InvalidMemberCount)
        );
        Ok(())
    }
}
