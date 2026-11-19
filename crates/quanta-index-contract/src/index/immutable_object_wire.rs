//! Pure immutable-object wire identity shared by the store and Native producers.

use super::{Digest, ObjectRef};
use sha2::{Digest as _, Sha256};
use std::convert::Infallible;
use std::fmt;
use std::io;

pub const NATIVE_IMMUTABLE_OBJECT_MAGIC_V1: &[u8; 8] = b"QIOBJ001";
pub const NATIVE_IMMUTABLE_OBJECT_HASH_DOMAIN_V1: &[u8] = b"quanta-index-immutable-object-v1\0";
pub const NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1: usize = 16;

const PROTECTED_WORK_CHECKPOINT_BYTES_V1: usize = 64 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub enum NativeProtectedEncodingErrorV1<E> {
    Work(E),
    Format(&'static str),
}

impl<E: fmt::Display> fmt::Display for NativeProtectedEncodingErrorV1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Work(cause) => write!(formatter, "work refused: {cause}"),
            Self::Format(message) => formatter.write_str(message),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for NativeProtectedEncodingErrorV1<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Work(cause) => Some(cause),
            Self::Format(_) => None,
        }
    }
}

impl<E> NativeProtectedEncodingErrorV1<E> {
    pub fn into_error(self, format: impl FnOnce(&'static str) -> E) -> E {
        match self {
            Self::Work(error) => error,
            Self::Format(message) => format(message),
        }
    }
}

impl NativeProtectedEncodingErrorV1<Infallible> {
    pub(super) fn into_format_error(self) -> &'static str {
        match self {
            Self::Work(never) => match never {},
            Self::Format(message) => message,
        }
    }
}

struct ProtectedWorkWriterV1<'a, W, F, E> {
    output: W,
    checkpoint: &'a mut F,
    written: usize,
    next_checkpoint: usize,
    refusal: Option<E>,
}

impl<W: io::Write, F: FnMut() -> Result<(), E>, E> io::Write
    for ProtectedWorkWriterV1<'_, W, F, E>
{
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.written == self.next_checkpoint {
            if let Err(error) = (self.checkpoint)() {
                self.refusal = Some(error);
                return Err(io::Error::other("protected object encoding work refused"));
            }
            self.next_checkpoint = self
                .next_checkpoint
                .checked_add(PROTECTED_WORK_CHECKPOINT_BYTES_V1)
                .ok_or_else(|| io::Error::other("protected object encoding work overflow"))?;
        }
        let length = bytes.len().min(self.next_checkpoint - self.written);
        let written = self.output.write(&bytes[..length])?;
        self.written = self
            .written
            .checked_add(written)
            .ok_or_else(|| io::Error::other("protected object encoding work overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub(super) fn serialize_protected_with_work_v1<W: io::Write, F: FnMut() -> Result<(), E>, E>(
    value: &impl serde::Serialize,
    output: W,
    checkpoint: &mut F,
    format: &'static str,
) -> Result<(), NativeProtectedEncodingErrorV1<E>> {
    checkpoint().map_err(NativeProtectedEncodingErrorV1::Work)?;
    let mut writer = ProtectedWorkWriterV1 {
        output,
        checkpoint,
        written: 0,
        next_checkpoint: PROTECTED_WORK_CHECKPOINT_BYTES_V1,
        refusal: None,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| {
        writer
            .refusal
            .take()
            .map(NativeProtectedEncodingErrorV1::Work)
            .unwrap_or(NativeProtectedEncodingErrorV1::Format(format))
    })
}

#[must_use]
pub fn native_immutable_object_header_v1(
    payload_bytes: u32,
    child_count: u32,
) -> [u8; NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1] {
    let mut header = [0; NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1];
    header[..8].copy_from_slice(NATIVE_IMMUTABLE_OBJECT_MAGIC_V1);
    header[8..12].copy_from_slice(&payload_bytes.to_be_bytes());
    header[12..].copy_from_slice(&child_count.to_be_bytes());
    header
}

/// Compute an immutable identity with the supplied child order. This only
/// describes bytes; the caller still pays for hash work and Q still checks
/// physical custody, registry rules and the source admission.
#[must_use]
pub fn native_immutable_object_ref_v1(payload: &[u8], children: &[ObjectRef]) -> Option<ObjectRef> {
    let mut digest = native_immutable_object_hasher_v1(payload.len(), children)?;
    digest.update(payload);
    Some(ObjectRef(Digest(digest.finalize().into())))
}

/// Compute the same immutable identity while yielding between payload chunks.
/// The caller supplies the checkpoint for its already-admitted byte work.
pub fn native_immutable_object_ref_with_work_v1<E>(
    payload: &[u8],
    children: &[ObjectRef],
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<Option<ObjectRef>, E> {
    checkpoint()?;
    let Some(mut digest) = native_immutable_object_hasher_v1(payload.len(), children) else {
        return Ok(None);
    };
    for chunk in payload.chunks(PROTECTED_WORK_CHECKPOINT_BYTES_V1) {
        checkpoint()?;
        digest.update(chunk);
    }
    Ok(Some(ObjectRef(Digest(digest.finalize().into()))))
}

pub(super) fn native_immutable_object_hasher_v1(
    payload_bytes: usize,
    children: &[ObjectRef],
) -> Option<Sha256> {
    let (Ok(payload_bytes), Ok(child_count)) =
        (u32::try_from(payload_bytes), u32::try_from(children.len()))
    else {
        return None;
    };
    let header = native_immutable_object_header_v1(payload_bytes, child_count);
    let mut digest = Sha256::new();
    digest.update(NATIVE_IMMUTABLE_OBJECT_HASH_DOMAIN_V1);
    digest.update(header);
    for child in children {
        digest.update((child.0).0);
    }
    Some(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_hash_preserves_wire_identity_and_interior_work_refusal() {
        let payload = vec![b'q'; PROTECTED_WORK_CHECKPOINT_BYTES_V1 + 1];
        let children = [ObjectRef(Digest([0x3c; 32])), ObjectRef(Digest([0xa7; 32]))];
        // SHA256 of the fixed domain, QIOBJ001, big-endian lengths, the two
        // child digests in order, and 65,537 bytes of ASCII q.
        let expected = ObjectRef(Digest([
            0x0c, 0x41, 0x67, 0x9e, 0xc1, 0xb6, 0xfc, 0x19, 0x1f, 0x1b, 0x37, 0x15, 0x1c, 0x26,
            0xb3, 0xe6, 0x93, 0xaa, 0x6d, 0x5c, 0x35, 0xa3, 0xd6, 0x61, 0x80, 0xea, 0x94, 0x64,
            0xf3, 0x6e, 0xd1, 0x44,
        ]));
        assert_eq!(
            native_immutable_object_ref_with_work_v1(&payload, &children, &mut || {
                Ok::<(), &'static str>(())
            }),
            Ok(Some(expected))
        );
        let mut checks = 0;
        assert_eq!(
            native_immutable_object_ref_with_work_v1(&payload, &children, &mut || {
                checks += 1;
                if checks == 3 {
                    Err("hash cancelled")
                } else {
                    Ok(())
                }
            }),
            Err("hash cancelled")
        );
        assert_eq!(checks, 3);
    }
}
