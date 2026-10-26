use super::{Error, FiniteSemanticReasonV3, SemanticReasonV3};

use alloc::{string::String, vec::Vec};

use ciborium_io::Read;
use ciborium_ll::{simple, tag, Decoder, Header};
use serde::{de, de::Deserializer as _, forward_to_deserialize_any};

trait Expected<E: de::Error> {
    fn expected(self, kind: &'static str) -> E;
}

impl<E: de::Error> Expected<E> for Header {
    #[inline]
    fn expected(self, kind: &'static str) -> E {
        de::Error::invalid_type(
            match self {
                Header::Positive(x) => de::Unexpected::Unsigned(x),
                Header::Negative(x) => {
                    de::Unexpected::Signed(i64::from_be_bytes(x.to_be_bytes()) ^ !0)
                }
                Header::Bytes(..) => de::Unexpected::Other("bytes"),
                Header::Text(..) => de::Unexpected::Other("string"),

                Header::Array(..) => de::Unexpected::Seq,
                Header::Map(..) => de::Unexpected::Map,

                Header::Tag(..) => de::Unexpected::Other("tag"),

                Header::Simple(simple::FALSE) => de::Unexpected::Bool(false),
                Header::Simple(simple::TRUE) => de::Unexpected::Bool(true),
                Header::Simple(simple::NULL) => de::Unexpected::Other("null"),
                Header::Simple(simple::UNDEFINED) => de::Unexpected::Other("undefined"),
                Header::Simple(..) => de::Unexpected::Other("simple"),

                Header::Float(x) => de::Unexpected::Float(x),
                Header::Break => de::Unexpected::Other("break"),
            },
            &kind,
        )
    }
}

struct Deserializer<'b, R: Read, S: SemanticReasonV3 = String> {
    decoder: Decoder<R>,
    scratch: &'b mut [u8],
    recurse: usize,
    semantic_reason: core::marker::PhantomData<S>,
    parser_allocation_v3: Option<&'b mut dyn FiniteParserAllocationAdmissionV3>,
}

/// Finite signal that the caller-owned parser allocation authority refused.
/// The authoritative cause remains with that owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FiniteParserAllocationRefusalV3;

impl core::fmt::Display for FiniteParserAllocationRefusalV3 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("controlled CBOR parser allocation refused")
    }
}

/// Allocation owner for the parser's own temporary text and byte buffers.
///
/// The owner must admit and reserve before each append, and retain
/// its grants until both the decoder and any value adopting a buffer are gone.
/// A refusal is kept by the caller; the decoder emits only a finite reason.
/// Visitor-owned allocations require their own admitted `DeserializeSeed`.
pub trait FiniteParserAllocationAdmissionV3 {
    /// Claim the shared attempt before framing callbacks or work checkpoints.
    /// A refused claim is terminal for every clone of the owner.
    fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Release the successful framing phase to the parser's decode begin.
    fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Start exactly one decode attempt before any parser or seed work. A
    /// refused or interrupted attempt must never be reused by a clone.
    fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Finish only after the decoded value and the final work checkpoint.
    fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// End the shared attempt after any decode error. This must not allocate,
    /// fail, replace the original cause, or release live grants. Every clone of
    /// the owner must refuse further backing operations after this call.
    fn abort_decode_v3(&mut self);

    /// Check a definite text header before a parser buffer or visitor is born.
    /// `None` represents an indefinite text header.
    fn check_text_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Check a definite byte-string header before any buffer or visitor work.
    fn check_bytes_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Check an array's declared element count before visiting the sequence.
    fn check_sequence_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Reserve room for `additional` UTF-8 bytes under the caller's grant.
    fn reserve_text_v3(
        &mut self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Reserve room for `additional` bytes under the caller's grant.
    fn reserve_bytes_v3(
        &mut self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3>;

    /// Lend caller-owned, admitted scratch for one definite text item that
    /// exceeds the fixed decoder buffer. Keep the backing through the whole
    /// decode attempt, including refusal and unwind. The callback cannot
    /// return a borrow of this scratch. Unsupported owners remain fail closed.
    fn with_borrowed_text_scratch_v3(
        &mut self,
        _length: usize,
        _visit: &mut dyn FnMut(&mut [u8]),
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        Err(FiniteParserAllocationRefusalV3)
    }
}

/// A decode attempt poisons the shared authority on every error and unwind.
/// The caller keeps the owner, so a successful value can retain its grants.
struct FiniteDecodeAttemptGuardV3<'a> {
    owner: &'a mut dyn FiniteParserAllocationAdmissionV3,
    complete: bool,
}

impl Drop for FiniteDecodeAttemptGuardV3<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.owner.abort_decode_v3();
        }
    }
}

impl<R: Read, S: SemanticReasonV3> Deserializer<'_, R, S>
where
    R::Error: core::fmt::Debug,
{
    fn visit_definite_text_v3<'de, V: de::Visitor<'de>>(
        &mut self,
        length: usize,
        offset: usize,
        visitor: V,
    ) -> Result<V::Value, Error<R::Error, S>> {
        if length <= self.scratch.len() {
            let text = self
                .scratch
                .get_mut(..length)
                .ok_or(Error::Syntax(offset))?;
            self.decoder.read_exact(text)?;
            let text = core::str::from_utf8(text).map_err(|_utf8_error| Error::Syntax(offset))?;
            return visitor.visit_str(text);
        }
        let Some(owner) = self.parser_allocation_v3.as_deref_mut() else {
            return Err(Header::Text(Some(length)).expected("str"));
        };
        let decoder = &mut self.decoder;
        let mut visitor = Some(visitor);
        let mut result = None;
        let status = owner.with_borrowed_text_scratch_v3(length, &mut |text| {
            if result.is_some() || text.len() != length {
                result = Some(Err(Error::Syntax(offset)));
                return;
            }
            result = Some((|| {
                decoder.read_exact(text)?;
                let text =
                    core::str::from_utf8(text).map_err(|_utf8_error| Error::Syntax(offset))?;
                visitor.take().ok_or(Error::Syntax(offset))?.visit_str(text)
            })());
        });
        match (status, result) {
            (_, Some(Err(cause))) => Err(cause),
            (Ok(()), Some(Ok(value))) => Ok(value),
            _ => Err(Error::semantic(
                None,
                "controlled CBOR borrowed text scratch refused",
            )),
        }
    }

    #[inline]
    fn check_text_length_v3(&mut self, length: Option<usize>) -> Result<(), Error<R::Error, S>> {
        if self
            .parser_allocation_v3
            .as_deref_mut()
            .is_some_and(|owner| owner.check_text_length_v3(length).is_err())
        {
            return Err(Error::semantic(None, "controlled CBOR text header refused"));
        }
        Ok(())
    }

    #[inline]
    fn check_bytes_length_v3(&mut self, length: Option<usize>) -> Result<(), Error<R::Error, S>> {
        if self
            .parser_allocation_v3
            .as_deref_mut()
            .is_some_and(|owner| owner.check_bytes_length_v3(length).is_err())
        {
            return Err(Error::semantic(
                None,
                "controlled CBOR bytes header refused",
            ));
        }
        Ok(())
    }

    #[inline]
    fn check_sequence_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), Error<R::Error, S>> {
        if self
            .parser_allocation_v3
            .as_deref_mut()
            .is_some_and(|owner| owner.check_sequence_length_v3(length).is_err())
        {
            return Err(Error::semantic(
                None,
                "controlled CBOR array header refused",
            ));
        }
        Ok(())
    }

    #[inline]
    fn recurse<V, F: FnOnce(&mut Self) -> Result<V, Error<R::Error, S>>>(
        &mut self,
        func: F,
    ) -> Result<V, Error<R::Error, S>> {
        if self.recurse == 0 {
            return Err(Error::RecursionLimitExceeded);
        }

        self.recurse = self.recurse.saturating_sub(1);
        let result = func(self);
        self.recurse = self.recurse.saturating_add(1);
        result
    }

    #[inline]
    fn integer(&mut self, mut header: Option<Header>) -> Result<(bool, u128), Error<R::Error, S>> {
        loop {
            let header = match header.take() {
                Some(h) => h,
                None => self.decoder.pull()?,
            };

            let neg = if let Header::Positive(x) = header {
                return Ok((false, x.into()));
            } else if let Header::Negative(x) = header {
                return Ok((true, x.into()));
            } else if let Header::Tag(tag) = header {
                match tag {
                    tag::BIGPOS => false,
                    tag::BIGNEG => true,
                    _ => continue,
                }
            } else {
                return Err(header.expected("integer"));
            };

            let mut buffer = [0u8; 16];
            let mut value = [0u8; 16];
            let mut index = 0usize;

            let item = self.decoder.pull()?;
            return if let Header::Bytes(len) = item {
                self.check_bytes_length_v3(len)?;
                let mut segments = self.decoder.bytes(len);
                while let Some(mut segment) = segments.pull()? {
                    while let Some(chunk) = segment.pull(&mut buffer)? {
                        for b in chunk {
                            match index {
                                16 => return Err(de::Error::custom("bigint too large")),
                                0 if *b == 0 => continue, // Skip leading zeros
                                _ => {
                                    let slot = value.get_mut(index).ok_or_else(|| {
                                        <Error<R::Error, S> as de::Error>::custom(
                                            "bigint too large",
                                        )
                                    })?;
                                    *slot = *b;
                                }
                            }

                            index = index.saturating_add(1);
                        }
                    }
                }

                value
                    .get_mut(..index)
                    .ok_or_else(|| <Error<R::Error, S> as de::Error>::custom("bigint too large"))?
                    .reverse();
                Ok((neg, u128::from_le_bytes(value)))
            } else {
                Err(item.expected("bytes"))
            };
        }
    }
}

impl<'de, R: Read, S: SemanticReasonV3> de::Deserializer<'de> for &mut Deserializer<'_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let header = self.decoder.pull()?;
        self.decoder.push(header);

        match header {
            Header::Positive(..) => self.deserialize_u64(visitor),
            Header::Negative(x) => match i64::try_from(x) {
                Ok(..) => self.deserialize_i64(visitor),
                Err(..) => self.deserialize_i128(visitor),
            },

            Header::Bytes(len) => match len {
                Some(len) if len <= self.scratch.len() => self.deserialize_bytes(visitor),
                _ => self.deserialize_byte_buf(visitor),
            },

            Header::Text(len) => match len {
                Some(len) if len <= self.scratch.len() => self.deserialize_str(visitor),
                _ => self.deserialize_string(visitor),
            },

            Header::Array(..) => self.deserialize_seq(visitor),
            Header::Map(..) => self.deserialize_map(visitor),

            Header::Tag(tag) => {
                let _: Header = self.decoder.pull()?;

                // Peek at the next item.
                let header = self.decoder.pull()?;
                self.decoder.push(header);

                // If it is bytes, capture the length.
                let len = if let Header::Bytes(x) = header {
                    x
                } else {
                    None
                };

                if let Some(len) = len {
                    if (tag == tag::BIGPOS || tag == tag::BIGNEG) && len <= 16 {
                        let result = match self.integer(Some(Header::Tag(tag)))? {
                            (false, raw) => return visitor.visit_u128(raw),
                            (true, raw) => i128::try_from(raw).map(|x| x ^ !0),
                        };

                        return match result {
                            Ok(x) => visitor.visit_i128(x),
                            Err(..) => Err(de::Error::custom("integer too large")),
                        };
                    }
                }
                self.recurse(|me| {
                    let access = crate::tag::TagAccess::new(me, Some(tag));
                    visitor.visit_enum(access)
                })
            }

            Header::Float(..) => self.deserialize_f64(visitor),

            Header::Simple(simple::FALSE | simple::TRUE) => self.deserialize_bool(visitor),
            Header::Simple(simple::NULL | simple::UNDEFINED) => self.deserialize_option(visitor),
            h @ Header::Simple(..) => Err(h.expected("known simple value")),

            h @ Header::Break => Err(h.expected("non-break")),
        }
    }

    #[inline]
    fn deserialize_bool<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let offset = self.decoder.offset();

            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if matches!(header, Header::Simple(simple::FALSE)) {
                return visitor.visit_bool(false);
            }
            if matches!(header, Header::Simple(simple::TRUE)) {
                return visitor.visit_bool(true);
            }
            return Err(Error::semantic(offset, "expected bool"));
        }
    }

    #[inline]
    fn deserialize_f32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_f64(visitor)
    }

    #[inline]
    fn deserialize_f64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Float(x) = header {
                return visitor.visit_f64(x);
            }
            return Err(header.expected("float"));
        }
    }

    fn deserialize_i8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let result = match self.integer(None)? {
            (false, raw) => i64::try_from(raw),
            (true, raw) => i64::try_from(raw).map(|x| x ^ !0),
        };

        match result {
            Ok(x) => visitor.visit_i64(x),
            Err(..) => Err(de::Error::custom("integer too large")),
        }
    }

    fn deserialize_i128<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let result = match self.integer(None)? {
            (false, raw) => i128::try_from(raw),
            (true, raw) => i128::try_from(raw).map(|x| x ^ !0),
        };

        match result {
            Ok(x) => visitor.visit_i128(x),
            Err(..) => Err(de::Error::custom("integer too large")),
        }
    }

    fn deserialize_u8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let result = match self.integer(None)? {
            (false, raw) => u64::try_from(raw),
            (true, ..) => return Err(de::Error::custom("unexpected negative integer")),
        };

        match result {
            Ok(x) => visitor.visit_u64(x),
            Err(..) => Err(de::Error::custom("integer too large")),
        }
    }

    fn deserialize_u128<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.integer(None)? {
            (false, raw) => visitor.visit_u128(raw),
            (true, ..) => Err(de::Error::custom("unexpected negative integer")),
        }
    }

    fn deserialize_char<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let offset = self.decoder.offset();
            let header = self.decoder.pull()?;

            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Text(Some(len)) = header {
                self.check_text_length_v3(Some(len))?;
                if len <= 4 {
                    let mut buf = [0u8; 4];
                    let text = buf.get_mut(..len).ok_or(Error::Syntax(offset))?;
                    self.decoder.read_exact(text)?;

                    return match core::str::from_utf8(text) {
                        Ok(s) => match (s.chars().next(), s.chars().count()) {
                            (Some(ch), 1) => visitor.visit_char(ch),
                            _ => Err(header.expected("char")),
                        },
                        Err(..) => Err(Error::Syntax(offset)),
                    };
                }
            }
            return Err(header.expected("char"));
        }
    }

    fn deserialize_str<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let offset = self.decoder.offset();

            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Text(Some(len)) = header {
                self.check_text_length_v3(Some(len))?;
                return self.visit_definite_text_v3(len, offset, visitor);
            }
            return Err(header.expected("str"));
        }
    }

    fn deserialize_string<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Text(len) = header {
                self.check_text_length_v3(len)?;
                let mut buffer = String::new();

                let allocation_v3 = &mut self.parser_allocation_v3;
                let mut segments = self.decoder.text(len);
                while let Some(mut segment) = segments.pull()? {
                    while let Some(chunk) = segment.pull(self.scratch)? {
                        if let Some(owner_v3) = allocation_v3.as_deref_mut() {
                            if owner_v3.reserve_text_v3(&mut buffer, chunk.len()).is_err() {
                                return Err(Error::semantic(
                                    None,
                                    "controlled CBOR text append refused",
                                ));
                            }
                            if buffer.capacity().saturating_sub(buffer.len()) < chunk.len() {
                                return Err(Error::semantic(
                                    None,
                                    "controlled CBOR text backing was not reserved",
                                ));
                            }
                        }
                        buffer.push_str(chunk);
                    }
                }

                return visitor.visit_string(buffer);
            }
            return Err(header.expected("string"));
        }
    }

    fn deserialize_bytes<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Bytes(Some(len)) = header {
                self.check_bytes_length_v3(Some(len))?;
                if len <= self.scratch.len() {
                    let bytes = self.scratch.get_mut(..len).ok_or_else(|| {
                        <Error<R::Error, S> as de::Error>::custom(
                            "invalid CBOR bytes scratch length",
                        )
                    })?;
                    self.decoder.read_exact(bytes)?;
                    return visitor.visit_bytes(bytes);
                }
            }
            if let Header::Array(len) = header {
                self.check_sequence_length_v3(len)?;
                return self.recurse(|me| {
                    let access = Access(me, len);
                    visitor.visit_seq(access)
                });
            }
            return Err(header.expected("bytes"));
        }
    }

    fn deserialize_byte_buf<V: de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Bytes(len) = header {
                self.check_bytes_length_v3(len)?;
                let mut buffer = Vec::new();

                let allocation_v3 = &mut self.parser_allocation_v3;
                let mut segments = self.decoder.bytes(len);
                while let Some(mut segment) = segments.pull()? {
                    while let Some(chunk) = segment.pull(self.scratch)? {
                        if let Some(owner_v3) = allocation_v3.as_deref_mut() {
                            if owner_v3.reserve_bytes_v3(&mut buffer, chunk.len()).is_err() {
                                return Err(Error::semantic(
                                    None,
                                    "controlled CBOR bytes append refused",
                                ));
                            }
                            if buffer.capacity().saturating_sub(buffer.len()) < chunk.len() {
                                return Err(Error::semantic(
                                    None,
                                    "controlled CBOR bytes backing was not reserved",
                                ));
                            }
                        }
                        buffer.extend_from_slice(chunk);
                    }
                }

                return visitor.visit_byte_buf(buffer);
            }
            if let Header::Array(len) = header {
                self.check_sequence_length_v3(len)?;
                return self.recurse(|me| {
                    let access = Access(me, len);
                    visitor.visit_seq(access)
                });
            }
            return Err(header.expected("byte buffer"));
        }
    }

    fn deserialize_seq<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Array(len) = header {
                self.check_sequence_length_v3(len)?;
                return self.recurse(|me| {
                    let access = Access(me, len);
                    visitor.visit_seq(access)
                });
            }
            if let Header::Bytes(len) = header {
                self.check_bytes_length_v3(len)?;
                self.check_sequence_length_v3(len)?;
                let mut buffer = Vec::new();

                let allocation_v3 = &mut self.parser_allocation_v3;
                let mut segments = self.decoder.bytes(len);
                while let Some(mut segment) = segments.pull()? {
                    while let Some(chunk) = segment.pull(self.scratch)? {
                        if let Some(owner_v3) = allocation_v3.as_deref_mut() {
                            if owner_v3.reserve_bytes_v3(&mut buffer, chunk.len()).is_err() {
                                return Err(Error::semantic(
                                    None,
                                    "controlled CBOR sequence bytes refused",
                                ));
                            }
                            if buffer.capacity().saturating_sub(buffer.len()) < chunk.len() {
                                return Err(Error::semantic(
                                    None,
                                    "controlled CBOR sequence backing was not reserved",
                                ));
                            }
                        }
                        buffer.extend_from_slice(chunk);
                    }
                }

                return visitor.visit_seq(BytesAccess::<R, S>(
                    0,
                    buffer,
                    core::marker::PhantomData,
                ));
            }
            return Err(header.expected("array"));
        }
    }

    fn deserialize_map<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Map(len) = header {
                return self.recurse(|me| {
                    let access = Access(me, len);
                    visitor.visit_map(access)
                });
            }
            return Err(header.expected("map"));
        }
    }

    fn deserialize_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }

    fn deserialize_tuple<V: de::Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_identifier<V: de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        loop {
            let offset = self.decoder.offset();

            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if let Header::Text(Some(len)) = header {
                self.check_text_length_v3(Some(len))?;
                return self.visit_definite_text_v3(len, offset, visitor);
            }
            if let Header::Bytes(Some(len)) = header {
                self.check_bytes_length_v3(Some(len))?;
                if len <= self.scratch.len() {
                    let bytes = self.scratch.get_mut(..len).ok_or(Error::Syntax(offset))?;
                    self.decoder.read_exact(bytes)?;
                    return visitor.visit_bytes(bytes);
                }
            }
            return Err(header.expected("str or bytes"));
        }
    }

    fn deserialize_ignored_any<V: de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    #[inline]
    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let header = self.decoder.pull()?;
        if matches!(header, Header::Simple(simple::UNDEFINED | simple::NULL)) {
            visitor.visit_none()
        } else {
            self.decoder.push(header);
            visitor.visit_some(self)
        }
    }

    #[inline]
    fn deserialize_unit<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if matches!(header, Header::Simple(simple::UNDEFINED | simple::NULL)) {
                return visitor.visit_unit();
            }
            return Err(header.expected("unit"));
        }
    }

    #[inline]
    fn deserialize_unit_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(visitor)
    }

    #[inline]
    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    #[inline]
    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        if name == "@@TAG@@" {
            let header = self.decoder.pull()?;
            let tag = if let Header::Tag(x) = header {
                Some(x)
            } else {
                self.decoder.push(header);
                None
            };

            return self.recurse(|me| {
                let access = crate::tag::TagAccess::new(me, tag);
                visitor.visit_enum(access)
            });
        }

        loop {
            let header = self.decoder.pull()?;
            if matches!(header, Header::Tag(..)) {
                continue;
            }
            if matches!(header, Header::Text(..)) {
                self.decoder.push(header);
            } else if !matches!(header, Header::Map(Some(1))) {
                return Err(header.expected("enum"));
            }

            return self.recurse(|me| {
                let access = Access(me, Some(0));
                visitor.visit_enum(access)
            });
        }
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        false
    }
}

struct Access<'a, 'b, R: Read, S: SemanticReasonV3>(&'a mut Deserializer<'b, R, S>, Option<usize>);

impl<'de, R: Read, S: SemanticReasonV3> de::SeqAccess<'de> for Access<'_, '_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn next_element_seed<U: de::DeserializeSeed<'de>>(
        &mut self,
        seed: U,
    ) -> Result<Option<U::Value>, Self::Error> {
        match self.1 {
            Some(0) => return Ok(None),
            Some(x) => self.1 = Some(x.saturating_sub(1)),
            None => {
                let header = self.0.decoder.pull()?;
                if matches!(header, Header::Break) {
                    return Ok(None);
                }
                self.0.decoder.push(header);
            }
        }

        seed.deserialize(&mut *self.0).map(Some)
    }

    #[inline]
    fn size_hint(&self) -> Option<usize> {
        self.1
    }
}

impl<'de, R: Read, S: SemanticReasonV3> de::MapAccess<'de> for Access<'_, '_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        match self.1 {
            Some(0) => return Ok(None),
            Some(x) => self.1 = Some(x.saturating_sub(1)),
            None => {
                let header = self.0.decoder.pull()?;
                if matches!(header, Header::Break) {
                    return Ok(None);
                }
                self.0.decoder.push(header);
            }
        }

        seed.deserialize(&mut *self.0).map(Some)
    }

    #[inline]
    fn next_value_seed<V: de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        seed.deserialize(&mut *self.0)
    }

    #[inline]
    fn size_hint(&self) -> Option<usize> {
        self.1
    }
}

impl<'de, R: Read, S: SemanticReasonV3> de::EnumAccess<'de> for Access<'_, '_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;
    type Variant = Self;

    #[inline]
    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(&mut *self.0)?;
        Ok((variant, self))
    }
}

impl<'de, R: Read, S: SemanticReasonV3> de::VariantAccess<'de> for Access<'_, '_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    #[inline]
    fn newtype_variant_seed<U: de::DeserializeSeed<'de>>(
        self,
        seed: U,
    ) -> Result<U::Value, Self::Error> {
        seed.deserialize(&mut *self.0)
    }

    #[inline]
    fn tuple_variant<V: de::Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.deserialize_any(visitor)
    }

    #[inline]
    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.deserialize_any(visitor)
    }
}

struct BytesAccess<R: Read, S: SemanticReasonV3>(usize, Vec<u8>, core::marker::PhantomData<(R, S)>);

impl<'de, R: Read, S: SemanticReasonV3> de::SeqAccess<'de> for BytesAccess<R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn next_element_seed<U: de::DeserializeSeed<'de>>(
        &mut self,
        seed: U,
    ) -> Result<Option<U::Value>, Self::Error> {
        use de::IntoDeserializer;

        if let Some(&byte) = self.1.get(self.0) {
            self.0 = self.0.saturating_add(1);
            seed.deserialize(byte.into_deserializer()).map(Some)
        } else {
            Ok(None)
        }
    }

    #[inline]
    fn size_hint(&self) -> Option<usize> {
        Some(self.1.len().saturating_sub(self.0))
    }
}

struct TagAccess<'a, 'b, R: Read, S: SemanticReasonV3>(&'a mut Deserializer<'b, R, S>, usize);

impl<'de, R: Read, S: SemanticReasonV3> de::Deserializer<'de> for &mut TagAccess<'_, '_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let offset = self.0.decoder.offset();

        if let Header::Tag(x) = self.0.decoder.pull()? {
            visitor.visit_u64(x)
        } else {
            Err(Error::semantic(offset, "expected tag"))
        }
    }

    forward_to_deserialize_any! {
        i8 i16 i32 i64 i128
        u8 u16 u32 u64 u128
        bool f32 f64
        char str string
        bytes byte_buf
        seq map
        struct tuple tuple_struct
        identifier ignored_any
        option unit unit_struct newtype_struct enum
    }
}

impl<'de, R: Read, S: SemanticReasonV3> de::SeqAccess<'de> for TagAccess<'_, '_, R, S>
where
    R::Error: core::fmt::Debug,
{
    type Error = Error<R::Error, S>;

    #[inline]
    fn next_element_seed<U: de::DeserializeSeed<'de>>(
        &mut self,
        seed: U,
    ) -> Result<Option<U::Value>, Self::Error> {
        self.1 = self.1.saturating_add(1);

        match self.1 {
            1 => seed.deserialize(self).map(Some),
            2 => seed.deserialize(&mut *self.0).map(Some),
            _ => Ok(None),
        }
    }

    #[inline]
    fn size_hint(&self) -> Option<usize> {
        Some(match self.1 {
            0 => 2,
            1 => 1,
            _ => 0,
        })
    }
}

/// Deserializes as CBOR from a type with [`impl
/// ciborium_io::Read`](ciborium_io::Read) using a 4KB buffer on the stack.
///
/// If you want to deserialize faster at the cost of more memory, consider using
/// [`from_reader_with_buffer`](from_reader_with_buffer) with a larger buffer,
/// for example 64KB.
#[inline]
pub fn from_reader<T: de::DeserializeOwned, R: Read>(reader: R) -> Result<T, Error<R::Error>>
where
    R::Error: core::fmt::Debug,
{
    let mut scratch = [0; 4096];
    from_reader_with_buffer(reader, &mut scratch)
}

/// Deserializes CBOR with caller-provided scratch memory.
///
/// The source implements [`ciborium_io::Read`].
#[inline]
pub fn from_reader_with_buffer<T: de::DeserializeOwned, R: Read>(
    reader: R,
    scratch_buffer: &mut [u8],
) -> Result<T, Error<R::Error>>
where
    R::Error: core::fmt::Debug,
{
    let mut reader = Deserializer::<R, String> {
        decoder: reader.into(),
        scratch: scratch_buffer,
        recurse: 256,
        semantic_reason: core::marker::PhantomData,
        parser_allocation_v3: None,
    };

    T::deserialize(&mut reader)
}

/// Deserializes CBOR with a specified recursion limit.
///
/// The source implements [`ciborium_io::Read`]. Inputs nested beyond the
/// specified limit result in [`Error::RecursionLimitExceeded`].
///
/// Set a high recursion limit at your own risk (of stack exhaustion)!
#[inline]
pub fn from_reader_with_recursion_limit<T: de::DeserializeOwned, R: Read>(
    reader: R,
    recurse_limit: usize,
) -> Result<T, Error<R::Error>>
where
    R::Error: core::fmt::Debug,
{
    let mut scratch = [0; 4096];

    let mut reader = Deserializer::<R, String> {
        decoder: reader.into(),
        scratch: &mut scratch,
        recurse: recurse_limit,
        semantic_reason: core::marker::PhantomData,
        parser_allocation_v3: None,
    };

    T::deserialize(&mut reader)
}

/// Decode with the same CBOR parser and a finite semantic error payload.
/// Caller-provided scratch and a work-charging `Read` owner govern native births.
#[inline]
fn from_reader_with_finite_error_policy_v3<T: de::DeserializeOwned, R: Read>(
    reader: R,
    scratch_buffer: &mut [u8],
) -> Result<T, Error<R::Error, FiniteSemanticReasonV3>>
where
    R::Error: core::fmt::Debug,
{
    let mut reader = Deserializer::<R, FiniteSemanticReasonV3> {
        decoder: reader.into(),
        scratch: scratch_buffer,
        recurse: 256,
        semantic_reason: core::marker::PhantomData,
        parser_allocation_v3: None,
    };
    T::deserialize(&mut reader)
}

/// A finite read failure preserving the caller's typed work refusal.
#[derive(Debug)]
pub enum FiniteReadErrorV3<E> {
    /// The original refusal returned by the caller before or after a read.
    Work(E),
    /// The requested bytes are absent from the input slice.
    EndOfInput,
}

struct WorkChargedSliceReaderV3<'a, E, V> {
    remaining: &'a [u8],
    visit: &'a mut V,
    error: core::marker::PhantomData<E>,
}

impl<E, V: FnMut(usize) -> Result<(), E>> Read for WorkChargedSliceReaderV3<'_, E, V> {
    type Error = FiniteReadErrorV3<E>;

    fn read_exact(&mut self, data: &mut [u8]) -> Result<(), Self::Error> {
        (self.visit)(0).map_err(FiniteReadErrorV3::Work)?;
        (self.visit)(data.len()).map_err(FiniteReadErrorV3::Work)?;
        if self.remaining.len() < data.len() {
            return Err(FiniteReadErrorV3::EndOfInput);
        }
        let (read, remaining) = self.remaining.split_at(data.len());
        data.copy_from_slice(read);
        self.remaining = remaining;
        (self.visit)(0).map_err(FiniteReadErrorV3::Work)
    }
}

/// Deserializes with finite semantic errors and charged reads.
///
/// The caller owns the scratch buffer and
/// must reserve typed output backing before invoking this function.
/// Scratch must hold at least one four-byte UTF-8 scalar; rejecting smaller
/// buffers also prevents zero-progress reads in the existing segment decoder.
#[inline]
pub fn from_slice_with_finite_error_and_work_v3<T, E, V>(
    bytes: &[u8],
    scratch: &mut [u8],
    visit: &mut V,
) -> Result<T, Error<FiniteReadErrorV3<E>, FiniteSemanticReasonV3>>
where
    T: de::DeserializeOwned,
    E: core::fmt::Debug,
    V: FnMut(usize) -> Result<(), E>,
{
    visit(0).map_err(|cause| Error::Io(FiniteReadErrorV3::Work(cause)))?;
    if scratch.len() < 4 {
        return Err(Error::semantic(
            None,
            "decoder scratch must hold one UTF-8 scalar",
        ));
    }
    let reader = WorkChargedSliceReaderV3 {
        remaining: bytes,
        visit,
        error: core::marker::PhantomData,
    };
    let value = from_reader_with_finite_error_policy_v3(reader, scratch)?;
    visit(0).map_err(|cause| Error::Io(FiniteReadErrorV3::Work(cause)))?;
    Ok(value)
}

/// Decodes with a caller-owned Serde seed and parser-buffer reservations.
///
/// The seed must separately admit every
/// allocation it performs in its visitors; this entrypoint only controls the
/// parser's owned text/byte-buffer growth paths and declared field lengths.
///
/// The admission owner must retain grants for any parser buffer adopted by the
/// seed for as long as that decoded value remains live. Neither this function
/// nor a successful parse transfers grant ownership automatically.
#[inline]
pub fn from_slice_with_finite_seed_and_allocation_v3<'de, T, E, V>(
    bytes: &'de [u8],
    scratch: &mut [u8],
    seed: T,
    visit: &mut V,
    parser_allocation: &mut dyn FiniteParserAllocationAdmissionV3,
) -> Result<T::Value, Error<FiniteReadErrorV3<E>, FiniteSemanticReasonV3>>
where
    T: de::DeserializeSeed<'de>,
    E: core::fmt::Debug,
    V: FnMut(usize) -> Result<(), E>,
{
    let mut attempt = FiniteDecodeAttemptGuardV3 {
        owner: parser_allocation,
        complete: false,
    };
    if attempt.owner.begin_decode_v3().is_err() {
        return Err(Error::semantic(
            None,
            "controlled CBOR decode attempt refused",
        ));
    }
    let result = (|| {
        visit(0).map_err(|cause| Error::Io(FiniteReadErrorV3::Work(cause)))?;
        if scratch.len() < 4 {
            return Err(Error::semantic(
                None,
                "decoder scratch must hold one UTF-8 scalar",
            ));
        }
        let reader = WorkChargedSliceReaderV3 {
            remaining: bytes,
            visit,
            error: core::marker::PhantomData,
        };
        let value = {
            let mut decoder = Deserializer::<_, FiniteSemanticReasonV3> {
                decoder: reader.into(),
                scratch,
                recurse: 256,
                semantic_reason: core::marker::PhantomData,
                parser_allocation_v3: Some(&mut *attempt.owner),
            };
            seed.deserialize(&mut decoder)?
        };
        visit(0).map_err(|cause| Error::Io(FiniteReadErrorV3::Work(cause)))?;
        Ok(value)
    })();
    if result.is_ok() {
        if attempt.owner.complete_decode_v3().is_err() {
            return Err(Error::semantic(
                None,
                "controlled CBOR decode completion refused",
            ));
        }
        attempt.complete = true;
    }
    result
}
