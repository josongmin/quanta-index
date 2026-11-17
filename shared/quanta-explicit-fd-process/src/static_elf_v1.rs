//! Narrow loader-free executable admission for installed solver images.

use crate::ExplicitFdProcessErrorV1;
use std::fs::File;
use std::os::unix::fs::FileExt;

const INVALID_V1: ExplicitFdProcessErrorV1 = ExplicitFdProcessErrorV1::InvalidStaticElfExecutable;
const MAX_IMAGE_BYTES_V1: u64 = 1024 * 1024 * 1024;

fn read_exact_at_v1(
    file_v1: &File,
    mut bytes_v1: &mut [u8],
    mut offset_v1: u64,
) -> Result<(), ExplicitFdProcessErrorV1> {
    while !bytes_v1.is_empty() {
        let count_v1 = file_v1
            .read_at(bytes_v1, offset_v1)
            .map_err(|_| INVALID_V1)?;
        if count_v1 == 0 {
            return Err(INVALID_V1);
        }
        offset_v1 = offset_v1
            .checked_add(u64::try_from(count_v1).map_err(|_| INVALID_V1)?)
            .ok_or(INVALID_V1)?;
        bytes_v1 = &mut bytes_v1[count_v1..];
    }
    Ok(())
}

fn little_u16_v1(bytes_v1: &[u8], offset_v1: usize) -> u16 {
    u16::from_le_bytes(
        bytes_v1[offset_v1..offset_v1 + 2]
            .try_into()
            .expect("fixed ELF field"),
    )
}

fn little_u32_v1(bytes_v1: &[u8], offset_v1: usize) -> u32 {
    u32::from_le_bytes(
        bytes_v1[offset_v1..offset_v1 + 4]
            .try_into()
            .expect("fixed ELF field"),
    )
}

fn little_u64_v1(bytes_v1: &[u8], offset_v1: usize) -> u64 {
    u64::from_le_bytes(
        bytes_v1[offset_v1..offset_v1 + 8]
            .try_into()
            .expect("fixed ELF field"),
    )
}

/// Validate the bytes behind a retained FD without moving its file offset.
/// This excludes interpreter and dynamic imports; it does not certify ambient
/// filesystem, clock, network, or randomness inputs of the executable.
pub fn verify_static_linux_x86_64_elf_v1(file_v1: &File) -> Result<(), ExplicitFdProcessErrorV1> {
    let metadata_v1 = file_v1.metadata().map_err(|_| INVALID_V1)?;
    let image_size_v1 = metadata_v1.len();
    if !metadata_v1.is_file() || !(64..=MAX_IMAGE_BYTES_V1).contains(&image_size_v1) {
        return Err(INVALID_V1);
    }
    let mut header_v1 = [0u8; 64];
    read_exact_at_v1(file_v1, &mut header_v1, 0)?;
    if &header_v1[..7] != b"\x7fELF\x02\x01\x01"
        || !matches!(header_v1[7], 0 | 3)
        || little_u16_v1(&header_v1, 16) != 2
        || little_u16_v1(&header_v1, 18) != 62
        || little_u32_v1(&header_v1, 20) != 1
        || little_u16_v1(&header_v1, 52) != 64
        || little_u16_v1(&header_v1, 54) != 56
    {
        return Err(INVALID_V1);
    }
    let entry_v1 = little_u64_v1(&header_v1, 24);
    let program_offset_v1 = little_u64_v1(&header_v1, 32);
    let program_count_v1 = usize::from(little_u16_v1(&header_v1, 56));
    let program_bytes_v1 = u64::try_from(program_count_v1.checked_mul(56).ok_or(INVALID_V1)?)
        .map_err(|_| INVALID_V1)?;
    if entry_v1 == 0
        || program_offset_v1 < 64
        || !(1..=128).contains(&program_count_v1)
        || program_offset_v1
            .checked_add(program_bytes_v1)
            .ok_or(INVALID_V1)?
            > image_size_v1
    {
        return Err(INVALID_V1);
    }
    let mut executable_entry_v1 = false;
    for index_v1 in 0..program_count_v1 {
        let offset_v1 = program_offset_v1
            .checked_add(u64::try_from(index_v1 * 56).map_err(|_| INVALID_V1)?)
            .ok_or(INVALID_V1)?;
        let mut program_v1 = [0u8; 56];
        read_exact_at_v1(file_v1, &mut program_v1, offset_v1)?;
        match little_u32_v1(&program_v1, 0) {
            2 | 3 => return Err(INVALID_V1),
            1 => {
                let file_offset_v1 = little_u64_v1(&program_v1, 8);
                let virtual_address_v1 = little_u64_v1(&program_v1, 16);
                let file_bytes_v1 = little_u64_v1(&program_v1, 32);
                let memory_bytes_v1 = little_u64_v1(&program_v1, 40);
                if file_bytes_v1 > memory_bytes_v1
                    || file_offset_v1
                        .checked_add(file_bytes_v1)
                        .ok_or(INVALID_V1)?
                        > image_size_v1
                {
                    return Err(INVALID_V1);
                }
                if little_u32_v1(&program_v1, 4) & 1 != 0
                    && virtual_address_v1 <= entry_v1
                    && virtual_address_v1
                        .checked_add(memory_bytes_v1)
                        .ok_or(INVALID_V1)?
                        > entry_v1
                {
                    executable_entry_v1 = true;
                }
            }
            _ => {}
        }
    }
    if !executable_entry_v1 {
        return Err(INVALID_V1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_interpreter_and_dynamic_program_headers_v1() {
        let mut image_v1 = vec![0u8; 0x100];
        image_v1[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        image_v1[16..18].copy_from_slice(&2u16.to_le_bytes());
        image_v1[18..20].copy_from_slice(&62u16.to_le_bytes());
        image_v1[20..24].copy_from_slice(&1u32.to_le_bytes());
        image_v1[24..32].copy_from_slice(&0x400080u64.to_le_bytes());
        image_v1[32..40].copy_from_slice(&64u64.to_le_bytes());
        image_v1[52..54].copy_from_slice(&64u16.to_le_bytes());
        image_v1[54..56].copy_from_slice(&56u16.to_le_bytes());
        image_v1[56..58].copy_from_slice(&2u16.to_le_bytes());
        image_v1[64..68].copy_from_slice(&1u32.to_le_bytes());
        image_v1[68..72].copy_from_slice(&5u32.to_le_bytes());
        image_v1[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        image_v1[96..104].copy_from_slice(&0x100u64.to_le_bytes());
        image_v1[104..112].copy_from_slice(&0x100u64.to_le_bytes());
        let mut file_v1 = tempfile::tempfile().expect("fixture file");
        file_v1.write_all(&image_v1).expect("fixture write");
        assert_eq!(verify_static_linux_x86_64_elf_v1(&file_v1), Ok(()));
        for forbidden_v1 in [2u32, 3u32] {
            file_v1
                .write_all_at(&forbidden_v1.to_le_bytes(), 120)
                .expect("forbidden program header write");
            assert_eq!(
                verify_static_linux_x86_64_elf_v1(&file_v1),
                Err(ExplicitFdProcessErrorV1::InvalidStaticElfExecutable),
            );
        }
    }
}
