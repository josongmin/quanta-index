// SPDX-License-Identifier: Apache-2.0

use ciborium::{de::from_reader, value::Value};
use rand::{rngs::StdRng, Rng, SeedableRng};

const ITERATIONS: usize = 128 * 1024;
const SEED: u64 = 0xc1b0_12a5_2026_1013;

#[test]
#[expect(
    clippy::print_stderr,
    reason = "a parser panic must emit the fixed seed, iteration and exact triggering bytes before resuming the panic"
)]
fn fuzz() {
    let mut rng = StdRng::seed_from_u64(SEED);
    let mut buffer = [0u8; 32];
    let mut rejected = 0usize;

    for iteration in 0..ITERATIONS {
        let len = rng.gen_range(0..buffer.len());
        let fill = buffer
            .get_mut(..len)
            .unwrap_or_else(|| panic!("fuzz length {len} exceeded the fixed buffer"));
        rng.fill(fill);
        let input = buffer
            .get(..len)
            .unwrap_or_else(|| panic!("fuzz length {len} exceeded the fixed buffer"));

        match std::panic::catch_unwind(|| from_reader::<Value, _>(input)) {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => rejected += 1, // Malformed random CBOR is expected.
            Err(payload) => {
                eprintln!(
                    "CBOR parser panic: seed={SEED:#x} iteration={iteration} bytes={}",
                    hex::encode(input)
                );
                std::panic::resume_unwind(payload);
            }
        }
    }

    assert!(
        rejected > 0,
        "fixed-seed smoke must include malformed input"
    );
}
