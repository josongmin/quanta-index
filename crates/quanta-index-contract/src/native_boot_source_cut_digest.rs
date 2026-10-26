//! Shared identity for the source roots pinned by a Native boot selection.

use sha2::{Digest as _, Sha256};

use crate::index::{Digest, SourceCut};

const DOMAIN: &[u8] = b"quanta-index-native-boot-source-cut-v1\0";

/// Hash the canonical `SourceCut` JSON under the Native boot domain. This is
/// exactly the identity authenticated by a boot receipt, not a source event or
/// a prepublication expected cut.
pub fn native_boot_source_cut_digest_v1(source: &SourceCut) -> Result<Digest, serde_json::Error> {
    match native_boot_source_cut_digest_with_work_v1(source, &mut |_| {
        Ok::<(), std::convert::Infallible>(())
    }) {
        Ok(digest) => Ok(digest),
        Err(NativeBootSourceCutDigestErrorV1::Encode(error)) => Err(error),
        Err(NativeBootSourceCutDigestErrorV1::Work(never)) => match never {},
    }
}

#[derive(Debug)]
pub enum NativeBootSourceCutDigestErrorV1<E> {
    Work(E),
    Encode(serde_json::Error),
}

/// Charge the domain and each canonical JSON write before hashing its bytes.
/// A refused callback stops serialization and retains its original error.
pub fn native_boot_source_cut_digest_with_work_v1<E>(
    source: &SourceCut,
    work: &mut impl FnMut(usize) -> Result<(), E>,
) -> Result<Digest, NativeBootSourceCutDigestErrorV1<E>> {
    struct HashWriter<'a, E, F> {
        hash: Sha256,
        work: &'a mut F,
        first_work_refusal: &'a mut Option<E>,
    }

    impl<E, F: FnMut(usize) -> Result<(), E>> std::io::Write for HashWriter<'_, E, F> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Err(cause) = (self.work)(bytes.len()) {
                *self.first_work_refusal = Some(cause);
                return Err(std::io::ErrorKind::Other.into());
            }
            self.hash.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    work(DOMAIN.len()).map_err(NativeBootSourceCutDigestErrorV1::Work)?;
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    let mut first_work_refusal = None;
    let mut writer = HashWriter {
        hash,
        work,
        first_work_refusal: &mut first_work_refusal,
    };
    let encoded = serde_json::to_writer(&mut writer, source);
    let HashWriter { hash, .. } = writer;
    if let Some(cause) = first_work_refusal {
        return Err(NativeBootSourceCutDigestErrorV1::Work(cause));
    }
    encoded.map_err(NativeBootSourceCutDigestErrorV1::Encode)?;
    Ok(Digest(hash.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::MapRoot;

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let (left, right) = (&$left, &$right);
            if left != right {
                return Err(format!("values differ: {left:?} != {right:?}").into());
            }
        }};
    }

    #[test]
    fn empty_cut_keeps_the_boot_digest_domain_and_canonical_field_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let cut = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        check_eq!(
            serde_json::to_string(&cut)?,
            r#"{"owners":null,"paths":null,"frontiers":null}"#,
        );
        check_eq!(
            native_boot_source_cut_digest_v1(&cut)?.0,
            [
                35, 113, 21, 26, 120, 7, 19, 217, 143, 224, 114, 212, 177, 71, 68, 66, 238, 235,
                249, 50, 57, 45, 76, 209, 178, 100, 184, 20, 137, 205, 131, 65,
            ],
        );
        let mut charged = 0_usize;
        let metered = native_boot_source_cut_digest_with_work_v1(&cut, &mut |bytes| {
            charged = charged.checked_add(bytes).ok_or("work count overflow")?;
            Ok::<(), &'static str>(())
        });
        let Ok(metered) = metered else {
            return Err("metered boot digest failed".into());
        };
        check_eq!(metered, native_boot_source_cut_digest_v1(&cut)?);
        check_eq!(
            Some(charged),
            DOMAIN.len().checked_add(serde_json::to_vec(&cut)?.len())
        );
        match native_boot_source_cut_digest_with_work_v1(&cut, &mut |_| Err("refused")) {
            Err(NativeBootSourceCutDigestErrorV1::Work("refused")) => {}
            _ => return Err("work refusal was not preserved".into()),
        }
        let mut calls = 0_usize;
        match native_boot_source_cut_digest_with_work_v1(&cut, &mut |_| {
            calls = calls.checked_add(1).ok_or("call count overflow")?;
            if calls == 2 {
                Err("JSON work refused")
            } else {
                Ok(())
            }
        }) {
            Err(NativeBootSourceCutDigestErrorV1::Work("JSON work refused")) => {}
            _ => return Err("JSON work refusal was not preserved".into()),
        }
        check_eq!(calls, 2);
        Ok(())
    }
}
