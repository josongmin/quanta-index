//! Quanta-owned finite CBOR decoder over the public `ciborium-ll` token reader.
//! The source is original product code; no Ciborium Serde parser is copied.

use ciborium_io::Read;
use ciborium_ll::{Decoder, Header, simple, tag};
use serde::de::{self, DeserializeSeed, Deserializer as _, IntoDeserializer, Visitor};
use serde::forward_to_deserialize_any;
use std::fmt::{self, Debug, Display};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FiniteParserAllocationRefusalV3;
impl Display for FiniteParserAllocationRefusalV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("controlled CBOR parser allocation refused")
    }
}

pub trait FiniteParserAllocationAdmissionV3 {
    fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn abort_decode_v3(&mut self);
    fn check_text_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn check_bytes_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn check_sequence_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn reserve_text_v3(
        &mut self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn reserve_bytes_v3(
        &mut self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;
    fn with_borrowed_text_scratch_v3(
        &mut self,
        _length: usize,
        _visit: &mut dyn FnMut(&mut [u8]),
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        Err(FiniteParserAllocationRefusalV3)
    }
}

#[derive(Debug)]
pub enum FiniteReadErrorV3<E> {
    Work(E),
    EndOfInput,
}
impl<E: Display> Display for FiniteReadErrorV3<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Work(e) => write!(f, "read work refused: {e}"),
            Self::EndOfInput => f.write_str("unexpected end of input"),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for FiniteReadErrorV3<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Work(e) => Some(e),
            Self::EndOfInput => None,
        }
    }
}

#[derive(Debug)]
pub enum FiniteSemanticReasonV3 {
    Static(&'static str),
    CustomRefused,
}
pub trait SemanticReasonV3: Debug + From<&'static str> {
    fn custom_v3<T: Display>(message: T) -> Self;
}
impl From<&'static str> for FiniteSemanticReasonV3 {
    fn from(reason: &'static str) -> Self {
        Self::Static(reason)
    }
}
impl SemanticReasonV3 for String {
    fn custom_v3<T: Display>(message: T) -> Self {
        message.to_string()
    }
}
impl SemanticReasonV3 for FiniteSemanticReasonV3 {
    fn custom_v3<T: Display>(_message: T) -> Self {
        Self::CustomRefused
    }
}
#[derive(Debug)]
pub enum Error<E, S = FiniteSemanticReasonV3> {
    Io(E),
    Syntax(usize),
    Semantic(Option<usize>, S),
    RecursionLimitExceeded,
}
impl<E, S> Error<E, S> {
    pub fn semantic(at: impl Into<Option<usize>>, why: impl Into<S>) -> Self {
        Self::Semantic(at.into(), why.into())
    }
}
impl<E, S> From<ciborium_ll::Error<E>> for Error<E, S> {
    fn from(e: ciborium_ll::Error<E>) -> Self {
        match e {
            ciborium_ll::Error::Io(e) => Self::Io(e),
            ciborium_ll::Error::Syntax(at) => Self::Syntax(at),
        }
    }
}
impl<E: Debug, S: Debug> Display for Error<E, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: Debug, S: Debug> std::error::Error for Error<E, S> {}
impl<E: Debug, S: SemanticReasonV3> de::Error for Error<E, S> {
    fn custom<T: Display>(message: T) -> Self {
        Self::Semantic(None, S::custom_v3(message))
    }
}

fn invalid_type<E, P: SemanticReasonV3>() -> Error<E, P> {
    Error::Semantic(None, P::custom_v3("invalid CBOR value"))
}

fn expected_header<E: Debug, P: SemanticReasonV3>(
    header: Header,
    expected: &'static str,
) -> Error<E, P> {
    let actual = match header {
        Header::Positive(value) => de::Unexpected::Unsigned(value),
        Header::Negative(value) => {
            de::Unexpected::Signed(i64::from_be_bytes(value.to_be_bytes()) ^ !0)
        }
        Header::Bytes(_) => de::Unexpected::Other("bytes"),
        Header::Text(_) => de::Unexpected::Other("string"),
        Header::Array(_) => de::Unexpected::Seq,
        Header::Map(_) => de::Unexpected::Map,
        Header::Tag(_) => de::Unexpected::Other("tag"),
        Header::Simple(simple::FALSE) => de::Unexpected::Bool(false),
        Header::Simple(simple::TRUE) => de::Unexpected::Bool(true),
        Header::Simple(simple::NULL) => de::Unexpected::Other("null"),
        Header::Simple(simple::UNDEFINED) => de::Unexpected::Other("undefined"),
        Header::Simple(_) => de::Unexpected::Other("simple"),
        Header::Float(value) => de::Unexpected::Float(value),
        Header::Break => de::Unexpected::Other("break"),
    };
    de::Error::invalid_type(actual, &expected)
}

struct ChargedReader<'a, E, F> {
    remaining: &'a [u8],
    work: &'a mut F,
    error: std::marker::PhantomData<E>,
}
impl<E, F: FnMut(usize) -> Result<(), E>> Read for ChargedReader<'_, E, F> {
    type Error = FiniteReadErrorV3<E>;
    fn read_exact(&mut self, out: &mut [u8]) -> Result<(), Self::Error> {
        (self.work)(0).map_err(FiniteReadErrorV3::Work)?;
        (self.work)(out.len()).map_err(FiniteReadErrorV3::Work)?;
        if self.remaining.len() < out.len() {
            return Err(FiniteReadErrorV3::EndOfInput);
        }
        let (part, rest) = self.remaining.split_at(out.len());
        out.copy_from_slice(part);
        self.remaining = rest;
        (self.work)(0).map_err(FiniteReadErrorV3::Work)
    }
}

struct Attempt<'a> {
    owner: &'a mut dyn FiniteParserAllocationAdmissionV3,
    complete: bool,
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.owner.abort_decode_v3();
        }
    }
}

pub fn from_slice_with_finite_seed_and_allocation_v3<'de, S, E, F>(
    bytes: &'de [u8],
    scratch: &mut [u8],
    seed: S,
    work: &mut F,
    owner: &mut dyn FiniteParserAllocationAdmissionV3,
) -> Result<S::Value, Error<FiniteReadErrorV3<E>>>
where
    S: DeserializeSeed<'de>,
    E: Debug,
    F: FnMut(usize) -> Result<(), E>,
{
    let mut attempt = Attempt {
        owner,
        complete: false,
    };
    attempt
        .owner
        .begin_decode_v3()
        .map_err(|_error| Error::semantic(None, "controlled CBOR decode attempt refused"))?;
    let result = (|| {
        work(0).map_err(|e| Error::Io(FiniteReadErrorV3::Work(e)))?;
        if scratch.len() < 4 {
            return Err(Error::semantic(
                None,
                "decoder scratch must hold one UTF-8 scalar",
            ));
        }
        let reader = ChargedReader {
            remaining: bytes,
            work,
            error: std::marker::PhantomData,
        };
        let mut decoder = FiniteDecoder {
            tokens: Decoder::from(reader),
            scratch,
            owner: Some(&mut *attempt.owner),
            depth: 0,
            definite_bound: None,
            reason: std::marker::PhantomData,
        };
        let value = seed.deserialize(&mut decoder)?;
        work(0).map_err(|e| Error::Io(FiniteReadErrorV3::Work(e)))?;
        Ok(value)
    })();
    if result.is_ok() {
        attempt
            .owner
            .complete_decode_v3()
            .map_err(|_error| Error::semantic(None, "controlled CBOR decode completion refused"))?;
        attempt.complete = true;
    }
    result
}

pub fn from_slice_with_finite_error_and_work_v3<T, E, F>(
    bytes: &[u8],
    scratch: &mut [u8],
    work: &mut F,
) -> Result<T, Error<FiniteReadErrorV3<E>>>
where
    T: de::DeserializeOwned,
    E: Debug,
    F: FnMut(usize) -> Result<(), E>,
{
    work(0).map_err(|e| Error::Io(FiniteReadErrorV3::Work(e)))?;
    if scratch.len() < 4 {
        return Err(Error::semantic(
            None,
            "decoder scratch must hold one UTF-8 scalar",
        ));
    }
    let reader = ChargedReader {
        remaining: bytes,
        work,
        error: std::marker::PhantomData,
    };
    let mut decoder = FiniteDecoder {
        tokens: Decoder::from(reader),
        scratch,
        owner: None,
        depth: 0,
        definite_bound: None,
        reason: std::marker::PhantomData,
    };
    let value = T::deserialize(&mut decoder)?;
    work(0).map_err(|e| Error::Io(FiniteReadErrorV3::Work(e)))?;
    Ok(value)
}

/// Ordinary slice decoding retains Serde's dynamic diagnostic text.
pub type SliceDecodeError<'a> = Error<<&'a [u8] as Read>::Error, String>;

/// Decode one value and return its consumed byte count. Definite owned scalars
/// reserve one backing after checking their length against the remaining input.
pub fn from_slice_with_definite_scalars<T: de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<(T, usize), SliceDecodeError<'_>> {
    let mut scratch = [0_u8; 4096];
    let mut decoder = FiniteDecoder {
        tokens: Decoder::from(bytes),
        scratch: &mut scratch,
        owner: None,
        depth: 0,
        definite_bound: Some(bytes.len()),
        reason: std::marker::PhantomData,
    };
    let value = T::deserialize(&mut decoder)?;
    Ok((value, decoder.tokens.offset()))
}

struct FiniteDecoder<'a, R: Read, P: SemanticReasonV3> {
    tokens: Decoder<R>,
    scratch: &'a mut [u8],
    owner: Option<&'a mut dyn FiniteParserAllocationAdmissionV3>,
    depth: usize,
    definite_bound: Option<usize>,
    reason: std::marker::PhantomData<P>,
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "unknown future CBOR token variants must use the generic typed refusal path"
)]
impl<R: Read, P: SemanticReasonV3> FiniteDecoder<'_, R, P>
where
    R::Error: Debug,
{
    fn pull(&mut self) -> Result<Header, Error<R::Error, P>> {
        self.tokens.pull().map_err(Into::into)
    }
    fn next_plain(&mut self) -> Result<Header, Error<R::Error, P>> {
        loop {
            match self.pull()? {
                Header::Tag(_) => {}
                header => return Ok(header),
            }
        }
    }
    fn step<T>(
        &mut self,
        run: impl FnOnce(&mut Self) -> Result<T, Error<R::Error, P>>,
    ) -> Result<T, Error<R::Error, P>> {
        if self.depth >= 256 {
            return Err(Error::RecursionLimitExceeded);
        }
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(Error::RecursionLimitExceeded)?;
        let result = run(self);
        self.depth = self.depth.saturating_sub(1);
        result
    }
    fn check_length(&mut self, length: Option<usize>, kind: u8) -> Result<(), Error<R::Error, P>> {
        if let (Some(bound), Some(length)) = (self.definite_bound, length)
            && kind <= 1
            && length > bound.saturating_sub(self.tokens.offset())
        {
            return Err(Error::Syntax(self.tokens.offset()));
        }
        self.check_borrowed_length(length, kind)
    }
    fn check_borrowed_length(
        &mut self,
        length: Option<usize>,
        kind: u8,
    ) -> Result<(), Error<R::Error, P>> {
        if let Some(owner) = self.owner.as_deref_mut() {
            let result = match kind {
                0 => Some(owner.check_text_length_v3(length)),
                1 => Some(owner.check_bytes_length_v3(length)),
                2 => Some(owner.check_sequence_length_v3(length)),
                _ => None,
            };
            if let Some(result) = result {
                let reason = match kind {
                    0 => "controlled CBOR text header refused",
                    1 => "controlled CBOR bytes header refused",
                    _ => "controlled CBOR array header refused",
                };
                result.map_err(|_error| Error::semantic(None, reason))?;
            }
        }
        Ok(())
    }
    fn read_text(&mut self, length: Option<usize>) -> Result<String, Error<R::Error, P>> {
        self.check_length(length, 0)?;
        let mut value = String::new();
        if self.definite_bound.is_some()
            && self.owner.is_none()
            && let Some(length) = length
        {
            value
                .try_reserve_exact(length)
                .map_err(|_| Error::semantic(None, "CBOR text backing allocation refused"))?;
        }
        let mut parts = self.tokens.text(length);
        while let Some(mut part) = parts.pull()? {
            while let Some(chunk) = part.pull(self.scratch)? {
                if let Some(owner) = self.owner.as_deref_mut() {
                    owner
                        .reserve_text_v3(&mut value, chunk.len())
                        .map_err(|_error| {
                            Error::semantic(None, "controlled CBOR text append refused")
                        })?;
                    if value.capacity().saturating_sub(value.len()) < chunk.len() {
                        return Err(Error::semantic(
                            None,
                            "controlled CBOR text backing was not reserved",
                        ));
                    }
                } else {
                    value.try_reserve(chunk.len()).map_err(|_error| {
                        Error::semantic(None, "CBOR text backing allocation refused")
                    })?;
                }
                value.push_str(chunk);
            }
        }
        Ok(value)
    }
    fn read_bytes(&mut self, length: Option<usize>) -> Result<Vec<u8>, Error<R::Error, P>> {
        self.check_length(length, 1)?;
        self.read_bytes_segments(length, false)
    }
    fn read_bytes_segments(
        &mut self,
        length: Option<usize>,
        sequence: bool,
    ) -> Result<Vec<u8>, Error<R::Error, P>> {
        let mut value = Vec::new();
        if self.definite_bound.is_some()
            && self.owner.is_none()
            && let Some(length) = length
        {
            value
                .try_reserve_exact(length)
                .map_err(|_| Error::semantic(None, "CBOR bytes backing allocation refused"))?;
        }
        let mut parts = self.tokens.bytes(length);
        while let Some(mut part) = parts.pull()? {
            while let Some(chunk) = part.pull(self.scratch)? {
                if let Some(owner) = self.owner.as_deref_mut() {
                    owner
                        .reserve_bytes_v3(&mut value, chunk.len())
                        .map_err(|_error| {
                            Error::semantic(
                                None,
                                if sequence {
                                    "controlled CBOR sequence bytes refused"
                                } else {
                                    "controlled CBOR bytes append refused"
                                },
                            )
                        })?;
                    if value.capacity().saturating_sub(value.len()) < chunk.len() {
                        return Err(Error::semantic(
                            None,
                            if sequence {
                                "controlled CBOR sequence backing was not reserved"
                            } else {
                                "controlled CBOR bytes backing was not reserved"
                            },
                        ));
                    }
                } else {
                    value.try_reserve(chunk.len()).map_err(|_error| {
                        Error::semantic(None, "CBOR bytes backing allocation refused")
                    })?;
                }
                value.extend_from_slice(chunk);
            }
        }
        Ok(value)
    }
    fn natural(&mut self) -> Result<(bool, u128), Error<R::Error, P>> {
        loop {
            match self.pull()? {
                Header::Positive(x) => return Ok((false, x.into())),
                Header::Negative(x) => return Ok((true, x.into())),
                Header::Tag(tag::BIGPOS) => return self.bigint(false),
                Header::Tag(tag::BIGNEG) => return self.bigint(true),
                Header::Tag(_) => {}
                header => return Err(expected_header(header, "integer")),
            }
        }
    }
    fn bigint(&mut self, negative: bool) -> Result<(bool, u128), Error<R::Error, P>> {
        let header = self.pull()?;
        let Header::Bytes(length) = header else {
            return Err(expected_header(header, "bytes"));
        };
        self.check_borrowed_length(length, 1)?;
        let mut value = 0_u128;
        let mut chunk = [0_u8; 16];
        let mut segments = self.tokens.bytes(length);
        while let Some(mut segment) = segments.pull()? {
            while let Some(bytes) = segment.pull(&mut chunk)? {
                for byte in bytes {
                    value = value
                        .checked_mul(256)
                        .and_then(|n| n.checked_add(u128::from(*byte)))
                        .ok_or_else(|| Error::semantic(None, P::custom_v3("bigint too large")))?;
                }
            }
        }
        Ok((negative, value))
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "unknown future CBOR token variants must use the generic typed refusal path"
)]
impl<'de, R: Read, P: SemanticReasonV3> de::Deserializer<'de> for &mut FiniteDecoder<'_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let header = self.pull()?;
        self.tokens.push(header);
        match header {
            Header::Positive(_) => self.deserialize_u64(visitor),
            Header::Negative(x) if i64::try_from(x).is_ok() => self.deserialize_i64(visitor),
            Header::Negative(_) => self.deserialize_i128(visitor),
            Header::Float(_) => self.deserialize_f64(visitor),
            Header::Simple(simple::FALSE | simple::TRUE) => self.deserialize_bool(visitor),
            Header::Simple(simple::NULL | simple::UNDEFINED) => self.deserialize_option(visitor),
            header @ Header::Simple(_) => Err(expected_header(header, "known simple value")),
            Header::Break => Err(expected_header(header, "non-break")),
            Header::Text(Some(n)) if n <= self.scratch.len() => self.deserialize_str(visitor),
            Header::Text(_) => self.deserialize_string(visitor),
            Header::Bytes(Some(n)) if n <= self.scratch.len() => self.deserialize_bytes(visitor),
            Header::Bytes(_) => self.deserialize_byte_buf(visitor),
            Header::Array(_) => self.deserialize_seq(visitor),
            Header::Map(_) => self.deserialize_map(visitor),
            Header::Tag(tag_value) => {
                let _: Header = self.pull()?;
                let next = self.pull()?;
                self.tokens.push(next);
                if (tag_value == tag::BIGPOS || tag_value == tag::BIGNEG)
                    && matches!(next, Header::Bytes(Some(n)) if n <= 16)
                {
                    let negative = tag_value == tag::BIGNEG;
                    let (_, value) = self.bigint(negative)?;
                    if !negative {
                        return visitor.visit_u128(value);
                    }
                    let signed = i128::try_from(value).map_err(|_error| invalid_type())? ^ !0;
                    return visitor.visit_i128(signed);
                }
                self.step(|parser| {
                    visitor.visit_enum(TagCursor {
                        parser: Some(parser),
                        tag: Some(tag_value),
                        index: 0,
                    })
                })
            }
        }
    }
    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let offset = self.tokens.offset();
            match self.pull()? {
                Header::Tag(_) => {}
                Header::Simple(simple::FALSE) => return visitor.visit_bool(false),
                Header::Simple(simple::TRUE) => return visitor.visit_bool(true),
                _ => return Err(Error::semantic(offset, "expected bool")),
            }
        }
    }
    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_f64(visitor)
    }
    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Float(x) => visitor.visit_f64(x),
            header => Err(expected_header(header, "float")),
        }
    }
    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let (negative, raw) = self.natural()?;
        let n = i64::try_from(raw)
            .map_err(|_error| Error::semantic(None, P::custom_v3("integer too large")))?;
        visitor.visit_i64(if negative { n ^ !0 } else { n })
    }
    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let (negative, raw) = self.natural()?;
        let n = i128::try_from(raw)
            .map_err(|_error| Error::semantic(None, P::custom_v3("integer too large")))?;
        visitor.visit_i128(if negative { n ^ !0 } else { n })
    }
    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.natural()? {
            (false, n) => visitor.visit_u64(
                u64::try_from(n)
                    .map_err(|_error| Error::semantic(None, P::custom_v3("integer too large")))?,
            ),
            _ => Err(Error::semantic(
                None,
                P::custom_v3("unexpected negative integer"),
            )),
        }
    }
    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.natural()? {
            (false, n) => visitor.visit_u128(n),
            _ => Err(Error::semantic(
                None,
                P::custom_v3("unexpected negative integer"),
            )),
        }
    }
    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let (offset, header) = loop {
            let offset = self.tokens.offset();
            let header = self.pull()?;
            if !matches!(header, Header::Tag(_)) {
                break (offset, header);
            }
        };
        match header {
            header @ Header::Text(Some(n)) if n <= 4 => {
                self.check_borrowed_length(Some(n), 0)?;
                let mut bytes = [0_u8; 4];
                let target = bytes
                    .get_mut(..n)
                    .ok_or_else(|| Error::Syntax(self.tokens.offset()))?;
                self.tokens.read_exact(target).map_err(Error::Io)?;
                let text = std::str::from_utf8(
                    bytes
                        .get(..n)
                        .ok_or_else(|| Error::Syntax(self.tokens.offset()))?,
                )
                .map_err(|_error| Error::Syntax(offset))?;
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => visitor.visit_char(ch),
                    _ => Err(expected_header(header, "char")),
                }
            }
            header => Err(expected_header(header, "char")),
        }
    }
    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let offset = self.tokens.offset();
        let Header::Text(Some(n)) = self.next_plain()? else {
            return Err(invalid_type());
        };
        self.check_borrowed_length(Some(n), 0)?;
        if n <= self.scratch.len() {
            let target = self.scratch.get_mut(..n).ok_or(Error::Syntax(offset))?;
            self.tokens.read_exact(target).map_err(Error::Io)?;
            let value = std::str::from_utf8(target).map_err(|_error| Error::Syntax(offset))?;
            return visitor.visit_str(value);
        }
        let Some(owner) = self.owner.as_deref_mut() else {
            return Err(invalid_type());
        };
        let decoder = &mut self.tokens;
        let mut visitor = Some(visitor);
        let mut result = None;
        owner
            .with_borrowed_text_scratch_v3(n, &mut |target| {
                result = Some((|| {
                    if target.len() != n {
                        return Err(Error::semantic(None, "invalid borrowed scratch length"));
                    }
                    decoder.read_exact(target).map_err(Error::Io)?;
                    let value =
                        std::str::from_utf8(target).map_err(|_error| Error::Syntax(offset))?;
                    visitor
                        .take()
                        .ok_or_else(|| Error::semantic(None, "repeated borrowed scratch callback"))?
                        .visit_str(value)
                })());
            })
            .map_err(|_error| {
                Error::semantic(None, "controlled CBOR borrowed text scratch refused")
            })?;
        result.ok_or_else(|| Error::semantic(None, "borrowed text callback missing"))?
    }
    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Text(n) => visitor.visit_string(self.read_text(n)?),
            header => Err(expected_header(header, "string")),
        }
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Bytes(Some(n)) if n <= self.scratch.len() => {
                self.check_borrowed_length(Some(n), 1)?;
                let target = self
                    .scratch
                    .get_mut(..n)
                    .ok_or_else(|| Error::Syntax(self.tokens.offset()))?;
                self.tokens.read_exact(target).map_err(Error::Io)?;
                visitor.visit_bytes(target)
            }
            Header::Array(n) => {
                self.check_length(n, 2)?;
                self.step(|parser| {
                    visitor.visit_seq(Items {
                        parser,
                        remaining: n,
                    })
                })
            }
            _ => Err(invalid_type()),
        }
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Bytes(n) => visitor.visit_byte_buf(self.read_bytes(n)?),
            Header::Array(n) => {
                self.check_length(n, 2)?;
                self.step(|parser| {
                    visitor.visit_seq(Items {
                        parser,
                        remaining: n,
                    })
                })
            }
            _ => Err(invalid_type()),
        }
    }
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Array(n) => {
                self.check_length(n, 2)?;
                self.step(|parser| {
                    visitor.visit_seq(Items {
                        parser,
                        remaining: n,
                    })
                })
            }
            Header::Bytes(n) => {
                self.check_length(n, 1)?;
                self.check_length(n, 2)?;
                let bytes = self.read_bytes_segments(n, true)?;
                visitor.visit_seq(ByteItems {
                    bytes,
                    index: 0,
                    error: std::marker::PhantomData,
                })
            }
            header => Err(expected_header(header, "array")),
        }
    }
    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Map(n) => {
                self.check_length(n, 3)?;
                self.step(|parser| {
                    visitor.visit_map(Items {
                        parser,
                        remaining: n,
                    })
                })
            }
            header => Err(expected_header(header, "map")),
        }
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }
    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _n: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _n: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Text(Some(n)) => {
                self.tokens.push(Header::Text(Some(n)));
                self.deserialize_str(visitor)
            }
            Header::Bytes(Some(n)) if n <= self.scratch.len() => {
                self.check_borrowed_length(Some(n), 1)?;
                let target = self
                    .scratch
                    .get_mut(..n)
                    .ok_or_else(|| Error::Syntax(self.tokens.offset()))?;
                self.tokens.read_exact(target).map_err(Error::Io)?;
                visitor.visit_bytes(target)
            }
            _ => Err(invalid_type()),
        }
    }
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let header = self.pull()?;
        if matches!(header, Header::Simple(simple::NULL | simple::UNDEFINED)) {
            visitor.visit_none()
        } else {
            self.tokens.push(header);
            visitor.visit_some(self)
        }
    }
    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.next_plain()? {
            Header::Simple(simple::NULL | simple::UNDEFINED) => visitor.visit_unit(),
            header => Err(expected_header(header, "unit")),
        }
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(visitor)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        if name == "@@TAG@@" {
            let h = self.pull()?;
            let tag = if let Header::Tag(n) = h {
                Some(n)
            } else {
                self.tokens.push(h);
                None
            };
            return self.step(|parser| {
                visitor.visit_enum(TagCursor {
                    parser: Some(parser),
                    tag,
                    index: 0,
                })
            });
        }
        let h = self.next_plain()?;
        match h {
            Header::Text(_) => self.tokens.push(h),
            Header::Map(Some(1)) => {}
            _ => return Err(invalid_type()),
        }
        self.step(|parser| {
            visitor.visit_enum(Items {
                parser,
                remaining: Some(0),
            })
        })
    }
    fn is_human_readable(&self) -> bool {
        false
    }
}

struct Items<'a, 'b, R: Read, P: SemanticReasonV3> {
    parser: &'a mut FiniteDecoder<'b, R, P>,
    remaining: Option<usize>,
}
impl<'de, R: Read, P: SemanticReasonV3> de::SeqAccess<'de> for Items<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        match self.remaining {
            Some(0) => return Ok(None),
            Some(n) => self.remaining = Some(n.saturating_sub(1)),
            None => {
                let h = self.parser.pull()?;
                if h == Header::Break {
                    return Ok(None);
                }
                self.parser.tokens.push(h);
            }
        }
        seed.deserialize(&mut *self.parser).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        self.remaining
    }
}
impl<'de, R: Read, P: SemanticReasonV3> de::MapAccess<'de> for Items<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        match self.remaining {
            Some(0) => return Ok(None),
            Some(n) => self.remaining = Some(n.saturating_sub(1)),
            None => {
                let h = self.parser.pull()?;
                if h == Header::Break {
                    return Ok(None);
                }
                self.parser.tokens.push(h);
            }
        }
        seed.deserialize(&mut *self.parser).map(Some)
    }
    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        seed.deserialize(&mut *self.parser)
    }
    fn size_hint(&self) -> Option<usize> {
        self.remaining
    }
}
impl<'de, R: Read, P: SemanticReasonV3> de::EnumAccess<'de> for Items<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    type Variant = Self;
    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let name = seed.deserialize(&mut *self.parser)?;
        Ok((name, self))
    }
}
impl<'de, R: Read, P: SemanticReasonV3> de::VariantAccess<'de> for Items<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        seed.deserialize(self.parser)
    }
    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.parser.deserialize_any(visitor)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.parser.deserialize_any(visitor)
    }
}

struct ByteItems<E, P: SemanticReasonV3> {
    bytes: Vec<u8>,
    index: usize,
    error: std::marker::PhantomData<(E, P)>,
}
impl<'de, E: Debug, P: SemanticReasonV3> de::SeqAccess<'de> for ByteItems<E, P> {
    type Error = Error<E, P>;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        let Some(&byte) = self.bytes.get(self.index) else {
            return Ok(None);
        };
        self.index = self.index.saturating_add(1);
        seed.deserialize(byte.into_deserializer()).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.bytes.len().saturating_sub(self.index))
    }
}

struct TagCursor<'a, 'b, R: Read, P: SemanticReasonV3> {
    parser: Option<&'a mut FiniteDecoder<'b, R, P>>,
    tag: Option<u64>,
    index: usize,
}
impl<'de, R: Read, P: SemanticReasonV3> de::Deserializer<'de> for &mut TagCursor<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.index = self.index.saturating_add(1);
        if self.index == 1 {
            visitor.visit_str(if self.tag.is_some() {
                "@@TAGGED@@"
            } else {
                "@@UNTAGGED@@"
            })
        } else {
            visitor.visit_u64(self.tag.ok_or_else(invalid_type)?)
        }
    }
    forward_to_deserialize_any! { bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier ignored_any }
}
impl<'de, R: Read, P: SemanticReasonV3> de::EnumAccess<'de> for TagCursor<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    type Variant = Self;
    fn variant_seed<S: DeserializeSeed<'de>>(
        mut self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(&mut self)?;
        Ok((variant, self))
    }
}
impl<'de, R: Read, P: SemanticReasonV3> de::VariantAccess<'de> for TagCursor<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn unit_variant(self) -> Result<(), Self::Error> {
        Err(invalid_type())
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        mut self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        seed.deserialize(self.parser.take().ok_or_else(invalid_type)?)
    }
    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(self)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(invalid_type())
    }
}
impl<'de, R: Read, P: SemanticReasonV3> de::SeqAccess<'de> for TagCursor<'_, '_, R, P>
where
    R::Error: Debug,
{
    type Error = Error<R::Error, P>;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.index = self.index.saturating_add(1);
        match self.index {
            2 => seed
                .deserialize(self.tag.ok_or_else(invalid_type)?.into_deserializer())
                .map(Some),
            3 => seed
                .deserialize(self.parser.take().ok_or_else(invalid_type)?)
                .map(Some),
            _ => Ok(None),
        }
    }
    fn size_hint(&self) -> Option<usize> {
        Some(3_usize.saturating_sub(self.index))
    }
}

#[cfg(test)]
mod fixture_decode {
    use serde::{
        Deserialize, Deserializer,
        de::{self, MapAccess, SeqAccess, Visitor},
    };
    use std::fmt;

    enum Field {
        A,
        B,
        Other,
    }

    impl<'de> Deserialize<'de> for Field {
        fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            struct FieldVisitor;
            impl Visitor<'_> for FieldVisitor {
                type Value = Field;

                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a fixture field name")
                }

                fn visit_str<E: de::Error>(self, value: &str) -> Result<Field, E> {
                    Ok(match value {
                        "a" => Field::A,
                        "b" => Field::B,
                        _ => Field::Other,
                    })
                }

                fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Field, E> {
                    Ok(match value {
                        b"a" => Field::A,
                        b"b" => Field::B,
                        _ => Field::Other,
                    })
                }
            }
            decoder.deserialize_identifier(FieldVisitor)
        }
    }

    struct FixtureVisitor<const HAS_B: bool>;

    impl<'de, const HAS_B: bool> Visitor<'de> for FixtureVisitor<HAS_B> {
        type Value = (u8, Option<(bool, String)>);

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a fixture struct")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut a = None;
            let mut b = None;
            while let Some(field) = map.next_key::<Field>()? {
                match field {
                    Field::A => {
                        if a.is_some() {
                            return Err(de::Error::duplicate_field("a"));
                        }
                        a = Some(map.next_value()?);
                    }
                    Field::B if HAS_B => {
                        if b.is_some() {
                            return Err(de::Error::duplicate_field("b"));
                        }
                        b = Some(map.next_value()?);
                    }
                    Field::B | Field::Other => {
                        let _: de::IgnoredAny = map.next_value()?;
                    }
                }
            }
            let a = a.ok_or_else(|| de::Error::missing_field("a"))?;
            if HAS_B && b.is_none() {
                return Err(de::Error::missing_field("b"));
            }
            Ok((a, b))
        }

        fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
            let a = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let b = if HAS_B {
                Some(
                    seq.next_element()?
                        .ok_or_else(|| de::Error::invalid_length(1, &self))?,
                )
            } else {
                None
            };
            if seq.next_element::<de::IgnoredAny>()?.is_some() {
                return Err(de::Error::invalid_length(usize::from(HAS_B) + 2, &self));
            }
            Ok((a, b))
        }
    }

    pub(super) fn decode<'de, D: Deserializer<'de>, const HAS_B: bool>(
        decoder: D,
        name: &'static str,
    ) -> Result<(u8, Option<(bool, String)>), D::Error> {
        const A_FIELDS: &[&str] = &["a"];
        const AB_FIELDS: &[&str] = &["a", "b"];
        decoder.deserialize_struct(
            name,
            if HAS_B { AB_FIELDS } else { A_FIELDS },
            FixtureVisitor::<HAS_B>,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Default)]
    pub(super) struct Owner {
        pub(super) state: u8,
        pub(super) births: usize,
        work: usize,
        refuse: bool,
        noop_backing: bool,
        pub(super) deny_header: Option<&'static str>,
        refuse_after: Option<usize>,
        borrowed: Vec<u8>,
    }
    impl FiniteParserAllocationAdmissionV3 for Owner {
        fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            self.state = 1;
            Ok(())
        }
        fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            self.state = 2;
            Ok(())
        }
        fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.state != 2 {
                return Err(FiniteParserAllocationRefusalV3);
            }
            self.state = 3;
            Ok(())
        }
        fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.state != 3 {
                return Err(FiniteParserAllocationRefusalV3);
            }
            self.state = 4;
            Ok(())
        }
        fn abort_decode_v3(&mut self) {
            self.state = 5;
        }
        fn check_text_length_v3(
            &mut self,
            _length: Option<usize>,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.deny_header == Some("text") {
                Err(FiniteParserAllocationRefusalV3)
            } else {
                Ok(())
            }
        }
        fn check_bytes_length_v3(
            &mut self,
            _length: Option<usize>,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.deny_header == Some("bytes") {
                Err(FiniteParserAllocationRefusalV3)
            } else {
                Ok(())
            }
        }
        fn check_sequence_length_v3(
            &mut self,
            _length: Option<usize>,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.deny_header == Some("sequence") {
                Err(FiniteParserAllocationRefusalV3)
            } else {
                Ok(())
            }
        }
        fn reserve_text_v3(
            &mut self,
            value: &mut String,
            additional: usize,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.refuse
                || self
                    .refuse_after
                    .is_some_and(|maximum| self.births >= maximum)
            {
                return Err(FiniteParserAllocationRefusalV3);
            }
            if self.noop_backing {
                return Ok(());
            }
            value
                .try_reserve_exact(additional)
                .map_err(|_error| FiniteParserAllocationRefusalV3)?;
            self.births = self.births.saturating_add(1);
            Ok(())
        }
        fn reserve_bytes_v3(
            &mut self,
            value: &mut Vec<u8>,
            additional: usize,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            if self.refuse
                || self
                    .refuse_after
                    .is_some_and(|maximum| self.births >= maximum)
            {
                return Err(FiniteParserAllocationRefusalV3);
            }
            if self.noop_backing {
                return Ok(());
            }
            value
                .try_reserve_exact(additional)
                .map_err(|_error| FiniteParserAllocationRefusalV3)?;
            self.births = self.births.saturating_add(1);
            Ok(())
        }
        fn with_borrowed_text_scratch_v3(
            &mut self,
            length: usize,
            visit: &mut dyn FnMut(&mut [u8]),
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            self.borrowed
                .try_reserve_exact(length)
                .map_err(|_error| FiniteParserAllocationRefusalV3)?;
            self.borrowed.resize(length, 0);
            visit(&mut self.borrowed);
            Ok(())
        }
    }
    pub(super) fn admit() -> Owner {
        let mut o = Owner::default();
        o.begin_preflight_v3().unwrap();
        o.complete_preflight_v3().unwrap();
        o
    }
    fn run<'de, S: DeserializeSeed<'de>>(
        wire: &'de [u8],
        seed: S,
        owner: &mut Owner,
    ) -> Result<S::Value, Error<FiniteReadErrorV3<()>>> {
        let mut scratch = [0_u8; 4];
        let mut work = 0_usize;
        let result = from_slice_with_finite_seed_and_allocation_v3(
            wire,
            &mut scratch,
            seed,
            &mut |n| {
                work = work.saturating_add(n);
                Ok(())
            },
            owner,
        );
        owner.work = owner.work.saturating_add(work);
        result
    }
    #[derive(Debug, PartialEq)]
    struct Fixture {
        a: u8,
        b: (bool, String),
    }
    impl<'de> Deserialize<'de> for Fixture {
        fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            let (a, b) = super::fixture_decode::decode::<_, true>(decoder, "Fixture")?;
            Ok(Self {
                a,
                b: b.ok_or_else(|| de::Error::missing_field("b"))?,
            })
        }
    }
    #[test]
    fn fixed_map_wire_and_retained_text() {
        let wire = [
            0xa2, 0x61, b'a', 0x01, 0x61, b'b', 0x82, 0xf5, 0x66, b'h', b'e', b'l', b'l', b'o',
            b'!',
        ];
        let mut owner = admit();
        let actual = run(&wire, std::marker::PhantomData::<Fixture>, &mut owner).unwrap();
        assert_eq!(
            actual,
            Fixture {
                a: 1,
                b: (true, "hello!".to_string())
            }
        );
        assert_eq!(owner.state, 4);
        assert_eq!(owner.births, 2);
        assert_eq!(owner.work, wire.len());
    }
    #[test]
    fn typed_byte_string_uses_parser_owner_and_seed() {
        let wire = [0x42, 1, 2];
        let mut owner = admit();
        let value = run(&wire, std::marker::PhantomData::<Vec<u8>>, &mut owner).unwrap();
        assert_eq!(value, [1, 2]);
        assert_eq!(owner.state, 4);
        assert_eq!(owner.births, 1);
        assert_eq!(owner.work, wire.len());
    }
    #[test]
    fn parser_refusal_poisoned_owner() {
        let wire = [0x66, b'h', b'e', b'l', b'l', b'o', b'!'];
        let mut owner = admit();
        owner.refuse = true;
        let result = run(&wire, std::marker::PhantomData::<String>, &mut owner);
        assert!(matches!(
            result,
            Err(Error::Semantic(
                _,
                FiniteSemanticReasonV3::Static("controlled CBOR text append refused")
            ))
        ));
        assert_eq!(owner.state, 5);
        assert_eq!(owner.births, 0);
    }
    #[test]
    fn denied_headers_abort_before_backing_birth() {
        for (wire, kind, reason) in [
            (
                &[0x61, b'a'][..],
                "text",
                "controlled CBOR text header refused",
            ),
            (
                &[0x41, 1][..],
                "bytes",
                "controlled CBOR bytes header refused",
            ),
            (
                &[0x81, 1][..],
                "sequence",
                "controlled CBOR array header refused",
            ),
        ] {
            let mut owner = admit();
            owner.deny_header = Some(kind);
            let result = run(wire, std::marker::PhantomData::<de::IgnoredAny>, &mut owner);
            assert!(matches!(
                result,
                Err(Error::Semantic(None, FiniteSemanticReasonV3::Static(actual)))
                    if actual == reason
            ));
            assert_eq!(owner.state, 5);
            assert_eq!(owner.births, 0);
        }
    }
    #[test]
    fn byte_sequence_refusal_keeps_its_static_reason() {
        let mut owner = admit();
        owner.refuse = true;
        let result = run(
            &[0x42, 1, 2],
            std::marker::PhantomData::<Vec<u8>>,
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("controlled CBOR sequence bytes refused")
            ))
        ));
        assert_eq!(owner.state, 5);
        assert_eq!(owner.births, 0);
    }
    #[test]
    fn noop_backing_cannot_allow_parser_growth() {
        let mut owner = admit();
        owner.noop_backing = true;
        let wire = [0x65, b'h', b'e', b'l', b'l', b'o'];
        let result = run(&wire, std::marker::PhantomData::<String>, &mut owner);
        assert!(matches!(
            result,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("controlled CBOR text backing was not reserved")
            ))
        ));
        assert_eq!(owner.state, 5);
        assert_eq!(owner.births, 0);
    }
    #[test]
    fn completed_and_aborted_attempts_refuse_reuse() {
        let mut completed = admit();
        assert_eq!(
            run(&[1], std::marker::PhantomData::<u8>, &mut completed).unwrap(),
            1
        );
        assert_eq!(completed.state, 4);
        let second = run(&[1], std::marker::PhantomData::<u8>, &mut completed);
        assert!(matches!(
            second,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("controlled CBOR decode attempt refused")
            ))
        ));
        assert_eq!(completed.state, 5);

        let mut aborted = admit();
        let first = run(&[0x18], std::marker::PhantomData::<u8>, &mut aborted);
        assert!(matches!(
            first,
            Err(Error::Io(FiniteReadErrorV3::EndOfInput))
        ));
        assert_eq!(aborted.state, 5);
        let second = run(&[1], std::marker::PhantomData::<u8>, &mut aborted);
        assert!(matches!(
            second,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("controlled CBOR decode attempt refused")
            ))
        ));
    }
    #[test]
    fn partial_backing_is_retained_after_later_refusal() {
        let wire = [0x66, b'h', b'e', b'l', b'l', b'o', b'!'];
        let mut owner = admit();
        owner.refuse_after = Some(1);
        let result = run(&wire, std::marker::PhantomData::<String>, &mut owner);
        assert!(matches!(
            result,
            Err(Error::Semantic(
                _,
                FiniteSemanticReasonV3::Static("controlled CBOR text append refused")
            ))
        ));
        assert_eq!(owner.births, 1);
        assert_eq!(owner.state, 5);
    }
    #[test]
    fn truncated_text_after_first_append_aborts_with_live_backing() {
        let mut owner = admit();
        let result = run(
            &[0x66, b'h', b'e', b'l', b'l'],
            std::marker::PhantomData::<String>,
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Io(FiniteReadErrorV3::EndOfInput))
        ));
        assert_eq!(owner.births, 1);
        assert_eq!(owner.state, 5);
    }
    struct TextLengthSeed;
    impl<'de> DeserializeSeed<'de> for TextLengthSeed {
        type Value = usize;
        fn deserialize<D: de::Deserializer<'de>>(
            self,
            decoder: D,
        ) -> Result<Self::Value, D::Error> {
            struct LengthVisitor;
            impl de::Visitor<'_> for LengthVisitor {
                type Value = usize;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("text")
                }
                fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
                    Ok(text.len())
                }
            }
            decoder.deserialize_str(LengthVisitor)
        }
    }
    #[test]
    fn borrowed_text_scratch_stays_with_owner() {
        let wire = [0x66, b'h', b'e', b'l', b'l', b'o', b'!'];
        let mut owner = admit();
        assert_eq!(run(&wire, TextLengthSeed, &mut owner).unwrap(), 6);
        assert_eq!(owner.borrowed, b"hello!");
        assert_eq!(owner.state, 4);
    }
    struct PanicSeed;
    impl<'de> DeserializeSeed<'de> for PanicSeed {
        type Value = ();
        #[expect(
            clippy::panic_in_result_fn,
            reason = "the panic fixture verifies that the decoder aborts an unwinding visitor"
        )]
        fn deserialize<D: de::Deserializer<'de>>(
            self,
            _decoder: D,
        ) -> Result<Self::Value, D::Error> {
            panic!("visitor panic")
        }
    }
    #[test]
    fn panic_aborts_owner() {
        let mut owner = admit();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&[0], PanicSeed, &mut owner)
        }));
        assert!(result.is_err());
        assert_eq!(owner.state, 5);
    }
    struct RejectAfterText;
    impl<'de> DeserializeSeed<'de> for RejectAfterText {
        type Value = String;
        fn deserialize<D: de::Deserializer<'de>>(
            self,
            decoder: D,
        ) -> Result<Self::Value, D::Error> {
            let _text = String::deserialize(decoder)?;
            Err(de::Error::custom("typed seed refused after parser birth"))
        }
    }
    #[test]
    fn typed_seed_refusal_aborts_after_backing_birth() {
        let mut owner = admit();
        let result = run(
            &[0x65, b'h', b'e', b'l', b'l', b'o'],
            RejectAfterText,
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Semantic(None, FiniteSemanticReasonV3::CustomRefused))
        ));
        assert_eq!(owner.state, 5);
        assert_eq!(owner.births, 2);
    }
    #[test]
    fn final_work_refusal_keeps_cause_and_aborts() {
        let mut owner = admit();
        let mut scratch = [0_u8; 4];
        let mut zero_calls = 0_usize;
        let result = from_slice_with_finite_seed_and_allocation_v3(
            &[1],
            &mut scratch,
            std::marker::PhantomData::<u8>,
            &mut |n| {
                if n == 0 {
                    zero_calls = zero_calls.saturating_add(1);
                    if zero_calls == 4 {
                        return Err("last checkpoint");
                    }
                }
                Ok(())
            },
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Io(FiniteReadErrorV3::Work("last checkpoint")))
        ));
        assert_eq!(owner.state, 5);
    }
    #[test]
    fn initial_work_refusal_precedes_parser_backing() {
        let mut owner = admit();
        let mut scratch = [0_u8; 4];
        let result = from_slice_with_finite_seed_and_allocation_v3(
            &[0x61, b'a'],
            &mut scratch,
            std::marker::PhantomData::<String>,
            &mut |_| Err::<(), _>("first work"),
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Io(FiniteReadErrorV3::Work("first work")))
        ));
        assert_eq!(owner.births, 0);
        assert_eq!(owner.state, 5);
    }
    #[test]
    fn short_scratch_aborts_before_parser_backing() {
        let mut owner = admit();
        let result = from_slice_with_finite_seed_and_allocation_v3(
            &[0x61, b'a'],
            &mut [],
            std::marker::PhantomData::<String>,
            &mut |_| Ok::<_, ()>(()),
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("decoder scratch must hold one UTF-8 scalar")
            ))
        ));
        assert_eq!(owner.births, 0);
        assert_eq!(owner.state, 5);
    }
    #[test]
    fn finite_work_error_is_original() {
        let mut scratch = [0_u8; 4];
        let mut work = |n| if n == 1 { Err("budget") } else { Ok(()) };
        let result: Result<u8, _> =
            from_slice_with_finite_error_and_work_v3(&[0x01], &mut scratch, &mut work);
        assert!(matches!(
            result,
            Err(Error::Io(FiniteReadErrorV3::Work("budget")))
        ));
    }
    #[test]
    fn scratch_refusal_preserves_first_work_cause() {
        let result: Result<u8, _> =
            from_slice_with_finite_error_and_work_v3(&[1], &mut [], &mut |_| Err("first"));
        assert!(matches!(
            result,
            Err(Error::Io(FiniteReadErrorV3::Work("first")))
        ));
    }
    #[test]
    fn empty_scratch_refuses_before_read_or_allocation() {
        let mut calls = 0;
        let result: Result<String, _> =
            from_slice_with_finite_error_and_work_v3(&[0x61, b'a'], &mut [], &mut |_| {
                calls += 1;
                Ok::<_, ()>(())
            });
        assert!(matches!(
            result,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("decoder scratch must hold one UTF-8 scalar")
            ))
        ));
        assert_eq!(calls, 1);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use serde::{Deserialize, Deserializer, de};

    #[test]
    fn definite_scalar_decoder_bounds_lengths_and_reports_consumed_bytes() {
        let (text, consumed): (String, usize) =
            from_slice_with_definite_scalars(&[0x62, b'o', b'k', 0x01]).unwrap();
        assert_eq!(text, "ok");
        assert_eq!(consumed, 3);
        let truncated: Result<(String, usize), _> =
            from_slice_with_definite_scalars(&[0x7a, 0xff, 0xff, 0xff, 0xff]);
        assert!(matches!(truncated, Err(Error::Syntax(5))));
        let truncated: Result<(Vec<u8>, usize), _> =
            from_slice_with_definite_scalars(&[0x5a, 0xff, 0xff, 0xff, 0xff]);
        assert!(matches!(truncated, Err(Error::Syntax(5))));
    }

    #[test]
    fn definite_owned_scalars_fit_their_declared_backing() {
        struct OwnedBytes(Vec<u8>);
        impl<'de> Deserialize<'de> for OwnedBytes {
            fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                struct BytesVisitor;
                impl Visitor<'_> for BytesVisitor {
                    type Value = OwnedBytes;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("an owned byte string")
                    }

                    fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<OwnedBytes, E> {
                        Ok(OwnedBytes(bytes))
                    }
                }
                decoder.deserialize_byte_buf(BytesVisitor)
            }
        }

        let payload = "é".repeat(2_501);
        let length = u16::try_from(payload.len()).unwrap().to_be_bytes();
        let mut wire = vec![0x79, length[0], length[1]];
        wire.extend(payload.as_bytes());
        let (text, consumed): (String, _) = from_slice_with_definite_scalars(&wire).unwrap();
        assert_eq!(text, payload);
        assert_eq!(consumed, wire.len());
        assert_eq!(text.capacity(), payload.len());

        wire[0] = 0x59;
        let (OwnedBytes(bytes), consumed) = from_slice_with_definite_scalars(&wire).unwrap();
        assert_eq!(bytes, payload.as_bytes());
        assert_eq!(consumed, wire.len());
        assert_eq!(bytes.capacity(), payload.len());
    }

    #[test]
    fn segmented_text_and_bytes_have_fixed_values() {
        let mut scratch = [0_u8; 4];
        let text: String = from_slice_with_finite_error_and_work_v3(
            &[0x7f, 0x62, b'h', b'e', 0x63, b'l', b'l', b'o', 0xff],
            &mut scratch,
            &mut |_| Ok::<_, ()>(()),
        )
        .unwrap();
        assert_eq!(text, "hello");
        let bytes: Vec<u8> = from_slice_with_finite_error_and_work_v3(
            &[0x5f, 0x42, 1, 2, 0x41, 3, 0xff],
            &mut scratch,
            &mut |_| Ok::<_, ()>(()),
        )
        .unwrap();
        assert_eq!(bytes, [1, 2, 3]);
    }
    #[test]
    fn byte_encoded_struct_key_and_indefinite_map() {
        #[derive(Debug, PartialEq)]
        struct Wire {
            a: u8,
        }
        impl<'de> Deserialize<'de> for Wire {
            fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let (a, _) = super::fixture_decode::decode::<_, false>(decoder, "Wire")?;
                Ok(Self { a })
            }
        }
        let mut scratch = [0_u8; 4];
        let value: Wire = from_slice_with_finite_error_and_work_v3(
            &[0xbf, 0x41, b'a', 0x01, 0xff],
            &mut scratch,
            &mut |_| Ok::<_, ()>(()),
        )
        .unwrap();
        assert_eq!(value, Wire { a: 1 });
    }
    #[test]
    fn declared_container_without_items_preserves_end_of_input() {
        let mut scratch = [0_u8; 4];
        for wire in [
            &[0x9a, 0xff, 0xff, 0xff, 0xff][..],
            &[0xba, 0xff, 0xff, 0xff, 0xff][..],
        ] {
            let result: Result<de::IgnoredAny, _> =
                from_slice_with_finite_error_and_work_v3(wire, &mut scratch, &mut |_| {
                    Ok::<_, ()>(())
                });
            assert!(matches!(
                result,
                Err(Error::Io(FiniteReadErrorV3::EndOfInput))
            ));
        }
    }
    #[test]
    fn recursion_limit_closes_nested_untrusted_input() {
        let mut wire = vec![0x81; 258];
        wire.push(0);
        let mut scratch = [0_u8; 4];
        let result: Result<de::IgnoredAny, _> =
            from_slice_with_finite_error_and_work_v3(&wire, &mut scratch, &mut |_| Ok::<_, ()>(()));
        assert!(matches!(result, Err(Error::RecursionLimitExceeded)));
    }
    struct FormattingMustNotRun;
    impl Display for FormattingMustNotRun {
        #[expect(
            clippy::panic_in_result_fn,
            reason = "the formatter panic proves finite semantic refusal does not format dynamic causes"
        )]
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("formatter invoked")
        }
    }
    struct CustomFailure;
    impl<'de> Deserialize<'de> for CustomFailure {
        fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
            Err(de::Error::custom(FormattingMustNotRun))
        }
    }
    #[test]
    fn semantic_refusal_never_formats_dynamic_cause() {
        let mut scratch = [0_u8; 4];
        let result: Result<CustomFailure, _> =
            from_slice_with_finite_error_and_work_v3(&[0], &mut scratch, &mut |_| Ok::<_, ()>(()));
        assert!(matches!(
            result,
            Err(Error::Semantic(None, FiniteSemanticReasonV3::CustomRefused))
        ));
    }

    #[test]
    fn ordinary_slice_decode_retains_custom_visitor_diagnostic() {
        struct Rejected;
        impl<'de> serde::Deserialize<'de> for Rejected {
            fn deserialize<D: serde::Deserializer<'de>>(_decoder: D) -> Result<Self, D::Error> {
                Err(serde::de::Error::custom("fixed visitor diagnostic"))
            }
        }
        let Err(error) = from_slice_with_definite_scalars::<Rejected>(&[0]) else {
            panic!("visitor must reject");
        };
        assert!(
            matches!(error, Error::Semantic(None, reason) if reason == "fixed visitor diagnostic")
        );
    }

    #[test]
    fn ordinary_slice_decode_preserves_invalid_scalar_reasons() {
        let negative = from_slice_with_definite_scalars::<u64>(&[0x20]);
        assert!(
            matches!(negative, Err(Error::Semantic(None, reason)) if reason == "unexpected negative integer")
        );
        let wrong_string = from_slice_with_definite_scalars::<String>(&[0]);
        assert!(
            matches!(wrong_string, Err(Error::Semantic(None, reason)) if reason == "invalid type: integer `0`, expected string")
        );
        let incomplete_bigint = from_slice_with_definite_scalars::<u64>(&[0xc2, 0x41]);
        assert!(matches!(incomplete_bigint, Err(Error::Io(_))));
        let wrong_float = from_slice_with_definite_scalars::<f64>(&[0]);
        assert!(
            matches!(wrong_float, Err(Error::Semantic(None, reason)) if reason == "invalid type: integer `0`, expected float")
        );
        let wrong_sequence = from_slice_with_definite_scalars::<Vec<u8>>(&[0]);
        assert!(
            matches!(wrong_sequence, Err(Error::Semantic(None, reason)) if reason == "invalid type: integer `0`, expected array")
        );
        let invalid_char = from_slice_with_definite_scalars::<char>(&[0x61, 0x80]);
        assert!(matches!(invalid_char, Err(Error::Syntax(0))));
        let wrong_unit = from_slice_with_definite_scalars::<()>(&[0]);
        assert!(
            matches!(wrong_unit, Err(Error::Semantic(None, reason)) if reason == "invalid type: integer `0`, expected unit")
        );
    }
}

#[cfg(test)]
mod fixed_wire_contract {
    use super::*;
    use serde::{Deserialize, de::DeserializeOwned};
    use std::collections::BTreeMap;

    // Literal CBOR wires and expected outcomes are fixed independently of the
    // decoder. These are a subset of 26 shapes compared with the previous fork
    // in a temporary differential harness; the tagged Value cases are not here.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "owned expected values make the fixed wire fixtures concise"
    )]
    fn accept<T: DeserializeOwned + Debug + PartialEq>(wire: &[u8], expected: T) {
        let mut scratch = [0_u8; 4];
        let mut charged = Vec::new();
        let actual: T = from_slice_with_finite_error_and_work_v3(wire, &mut scratch, &mut |n| {
            charged.push(n);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(charged.iter().sum::<usize>(), wire.len());
        assert_eq!(charged.first(), Some(&0));
        assert_eq!(charged.last(), Some(&0));
    }

    #[derive(Debug, PartialEq)]
    struct MapWire {
        a: u8,
        b: (bool, String),
    }
    impl<'de> Deserialize<'de> for MapWire {
        fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            let (a, b) = super::fixture_decode::decode::<_, true>(decoder, "MapWire")?;
            Ok(Self {
                a,
                b: b.ok_or_else(|| de::Error::missing_field("b"))?,
            })
        }
    }

    #[test]
    fn fixed_scalar_and_collection_wires() {
        accept(&[0x01], 1_u8);
        accept(&[0x20], -1_i8);
        accept(&[0xf9, 0x3c, 0x00], 1.0_f64);
        accept(&[0xf6], None::<u8>);
        accept(&[0xc2, 0x49, 0, 0, 0, 0, 0, 0, 0, 0, 1], 1_u128);
        accept(&[0xc3, 0x41, 1], -2_i128);
        accept(
            &[0x66, b'h', b'e', b'l', b'l', b'o', b'!'],
            "hello!".to_owned(),
        );
        accept(
            &[0x7f, 0x62, b'h', b'e', 0x63, b'l', b'l', b'o', 0xff],
            "hello".to_owned(),
        );
        accept(&[0x43, 1, 2, 3], vec![1_u8, 2, 3]);
        accept(&[0x5f, 0x42, 1, 2, 0x41, 3, 0xff], vec![1_u8, 2, 3]);
        accept(&[0x83, 1, 2, 3], vec![1_u8, 2, 3]);
        accept(&[0x9f, 1, 2, 0xff], vec![1_u8, 2]);
        accept(
            &[
                0xa2, 0x61, b'a', 1, 0x61, b'b', 0x82, 0xf5, 0x66, b'h', b'e', b'l', b'l', b'o',
                b'!',
            ],
            MapWire {
                a: 1,
                b: (true, "hello!".to_owned()),
            },
        );
        accept(
            &[0xbf, 0x61, b'a', 1, 0xff],
            BTreeMap::from([("a".to_owned(), 1_u8)]),
        );
    }

    fn refuse<T: DeserializeOwned>(
        wire: &[u8],
        expected: fn(&Error<FiniteReadErrorV3<()>>) -> bool,
    ) {
        let mut scratch = [0_u8; 4];
        let result: Result<T, _> =
            from_slice_with_finite_error_and_work_v3(wire, &mut scratch, &mut |_| Ok::<_, ()>(()));
        assert!(result.as_ref().is_err_and(expected));
    }

    #[test]
    fn fixed_malformed_wires_keep_typed_failures() {
        fn end(error: &Error<FiniteReadErrorV3<()>>) -> bool {
            matches!(error, Error::Io(FiniteReadErrorV3::EndOfInput))
        }
        fn syntax(error: &Error<FiniteReadErrorV3<()>>) -> bool {
            matches!(error, Error::Syntax(_))
        }
        refuse::<u8>(&[0x18], end);
        refuse::<Vec<u8>>(&[0x82, 1], end);
        refuse::<String>(&[0x64, b'a'], end);
        refuse::<Vec<u8>>(&[0x42, 1], end);
        refuse::<String>(&[0x7f, 0x61, b'a'], end);
        refuse::<Vec<u8>>(&[0x5f, 0x41, 1], end);
        refuse::<String>(&[0x62, 0xc3, 0x28], syntax);
        refuse::<MapWire>(&[0xa1, 0x61, b'a'], end);
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn oversized_container_lengths_are_syntax_errors() {
        // Each definite length is 2^32, which cannot fit a 32-bit usize.
        for major in [0x5b, 0x7b, 0x9b, 0xbb] {
            let wire = [major, 0, 0, 0, 1, 0, 0, 0, 0];
            refuse::<serde::de::IgnoredAny>(&wire, |error| matches!(error, Error::Syntax(_)));
        }
    }

    #[test]
    fn one_value_api_leaves_trailing_bytes_to_caller() {
        let mut scratch = [0_u8; 4];
        let value: u8 =
            from_slice_with_finite_error_and_work_v3(&[1, 2], &mut scratch, &mut |_| {
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(value, 1);
        assert_eq!(
            from_slice_with_definite_scalars::<u8>(&[1, 2]).unwrap().1,
            1
        );
    }

    struct AnyBytesRoute;
    impl<'de> DeserializeSeed<'de> for AnyBytesRoute {
        type Value = &'static str;
        fn deserialize<D: de::Deserializer<'de>>(
            self,
            decoder: D,
        ) -> Result<Self::Value, D::Error> {
            struct RouteVisitor;
            impl de::Visitor<'_> for RouteVisitor {
                type Value = &'static str;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a byte string")
                }
                fn visit_bytes<E: de::Error>(self, _: &[u8]) -> Result<Self::Value, E> {
                    Ok("borrowed")
                }
                fn visit_byte_buf<E: de::Error>(self, _: Vec<u8>) -> Result<Self::Value, E> {
                    Ok("owned")
                }
            }
            decoder.deserialize_any(RouteVisitor)
        }
    }

    #[test]
    fn empty_definite_bytes_use_borrowed_route_and_exact_work_trace() {
        let mut owner = super::tests::admit();
        let mut scratch = [0_u8; 4];
        let mut trace = Vec::new();
        let value = from_slice_with_finite_seed_and_allocation_v3(
            &[0x40, 0x00],
            &mut scratch,
            AnyBytesRoute,
            &mut |n| {
                trace.push(n);
                Ok::<_, ()>(())
            },
            &mut owner,
        )
        .unwrap();
        assert_eq!(value, "borrowed");
        assert_eq!(owner.state, 4);
        assert_eq!(owner.births, 0);
        assert_eq!(trace, [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn definite_bytes_visit_borrowed_scratch_without_parser_reservation() {
        let mut owner = super::tests::admit();
        let mut scratch = [0_u8; 4];
        let value = from_slice_with_finite_seed_and_allocation_v3(
            &[0x42, 1, 2],
            &mut scratch,
            AnyBytesRoute,
            &mut |_| Ok::<_, ()>(()),
            &mut owner,
        )
        .unwrap();
        assert_eq!(value, "borrowed");
        assert_eq!(owner.births, 0);
        assert_eq!(owner.state, 4);
    }

    #[test]
    fn borrowed_bytes_header_denial_precedes_visitor() {
        let mut owner = super::tests::admit();
        owner.deny_header = Some("bytes");
        let mut scratch = [0_u8; 4];
        let result = from_slice_with_finite_seed_and_allocation_v3(
            &[0x42, 1, 2],
            &mut scratch,
            AnyBytesRoute,
            &mut |_| Ok::<_, ()>(()),
            &mut owner,
        );
        assert!(matches!(
            result,
            Err(Error::Semantic(
                None,
                FiniteSemanticReasonV3::Static("controlled CBOR bytes header refused")
            ))
        ));
        assert_eq!(owner.births, 0);
        assert_eq!(owner.state, 5);
    }

    #[test]
    fn unsupported_simple_value_has_finite_custom_refusal() {
        let mut scratch = [0_u8; 4];
        let result: Result<de::IgnoredAny, _> =
            from_slice_with_finite_error_and_work_v3(&[0xe0], &mut scratch, &mut |_| {
                Ok::<_, ()>(())
            });
        assert!(matches!(
            result,
            Err(Error::Semantic(None, FiniteSemanticReasonV3::CustomRefused))
        ));
    }
}
