const CONTINUE_BIT: u8 = 128u8;

pub(crate) fn deserialize_read_checked(buf: &[u8]) -> std::io::Result<(usize, u64)> {
    let mut value = 0u64;
    for (index, &byte) in buf.iter().take(10).enumerate() {
        if index == 9 && byte > 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SSTable vint overflow",
            ));
        }
        value |= u64::from(byte & 127) << (index * 7);
        if byte < CONTINUE_BIT {
            return Ok((index + 1, value));
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "unterminated SSTable vint",
    ))
}

pub fn serialize(mut val: u64, buffer: &mut [u8]) -> usize {
    for (i, b) in buffer.iter_mut().enumerate() {
        let next_byte: u8 = (val & 127u64) as u8;
        val >>= 7;
        if val == 0u64 {
            *b = next_byte;
            return i + 1;
        } else {
            *b = next_byte | CONTINUE_BIT;
        }
    }
    10 //< actually unreachable
}

pub fn serialize_into_vec(val: u64, buffer: &mut Vec<u8>) {
    let mut buf = [0u8; 10];
    let num_bytes = serialize(val, &mut buf[..]);
    buffer.extend_from_slice(&buf[..num_bytes]);
}

// super slow but we don't care
pub fn deserialize_read(buf: &[u8]) -> (usize, u64) {
    let mut result = 0u64;
    let mut shift = 0u64;
    let mut consumed = 0;

    for &b in buf {
        consumed += 1;
        result |= u64::from(b % 128u8) << shift;
        if b < CONTINUE_BIT {
            break;
        }
        shift += 7;
    }
    (consumed, result)
}

#[cfg(test)]
mod tests {
    use std::u64;

    use super::{deserialize_read, serialize};

    fn aux_test_int(val: u64, expect_len: usize) {
        let mut buffer = [0u8; 14];
        assert_eq!(serialize(val, &mut buffer[..]), expect_len);
        assert_eq!(deserialize_read(&buffer), (expect_len, val));
    }

    #[test]
    fn test_vint() {
        aux_test_int(0u64, 1);
        aux_test_int(17u64, 1);
        aux_test_int(127u64, 1);
        aux_test_int(128u64, 2);
        aux_test_int(123423418u64, 4);
        for i in 1..63 {
            let power_of_two = 1u64 << i;
            aux_test_int(power_of_two + 1, (i / 7) + 1);
            aux_test_int(power_of_two, (i / 7) + 1);
            aux_test_int(power_of_two - 1, ((i - 1) / 7) + 1);
        }
        aux_test_int(u64::MAX, 10);
    }
}
