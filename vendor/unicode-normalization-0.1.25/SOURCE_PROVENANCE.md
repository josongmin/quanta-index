# unicode-normalization source provenance

- Upstream crate: unicode-normalization 0.1.25 (Unicode 17.0.0).
- Cached crates.io archive SHA256: `5fd4f6878c9cb28d874b009da9e8d183b5abc80117c40bbd187a1fde336be6e8`.
- Original decomposition tables, lookup functions and canonical recomposition algorithm retained. Ordinary iteration retains the upstream stable sort; controlled long-text iteration uses the stable admitted ordering described below.

The Quanta Index copy retains the canonical extension's build sources, tests and
licenses. The unused upstream `scripts/unicode.py` table generator is omitted;
the Unicode 17.0.0 range and lookup data retain the pinned archive contents.
The local `is_public_assigned` Rust wrapper uses `matches!` over the same scalar
patterns; this lint rewrite changes `src/tables.rs` source text.

## Local workspace lint adaptations

- Cargo metadata declares the `text-processing` category.
- The stable workspace fork exposes its library and explicit native-scratch test target. The retained upstream benchmark source requires nightly libtest and its omitted `benches/long.txt` input, so the local manifest does not register that unsupported target.
- Allocation-probe TLS unavailability aborts the test process; it cannot produce a successful false/zero observation. The probe uses const `Cell` keys without destructors.
- Hangul decomposition documents its checked syllable-range safety precondition.
- Hangul composition uses `is_multiple_of(T_COUNT)` for the same nonzero constant divisor.
- `is_public_assigned` uses `matches!` with unchanged Unicode scalar patterns.
- The archive digests below identify upstream files; locally adapted source text has its own repository identity.

## Local producer extension

- The same Decompositions/Recompositions steps delegate to ordinary or controlled scratch policy; no second Unicode table or NFC state machine.
- `quanta-native-scratch-v1` exposes borrowed `try_is_nfc_with_native_admission_v1` for the existing 512 UTF8-byte identity contract.
- `try_for_each_nfc_with_native_admission_v1` streams canonical scalars for caller-bounded long text. F15 uses it for exact output-length census and construction, admitting iterator backing and output Strings before birth.
- Actual TinyVec full-capacity doubling uses fallible reserve under before-birth admission; old/replacement backing is retained across growth. Callback protocol and observed capacity are checked.
- Original typed caller error, native allocation failure, invalid callback/capacity and unsupported producer are distinct. Scratch release follows both iterator buffer drops on every return.
- Work is consumed before iterator/state steps, decomposition, emitted scalar, canonical ordering batch, buffer movement and comparison. Long controlled ordering admits three linear passes before sorting; these are policy units rather than standard-library comparison counts.
- Pinned Rust 1.92, 64-bit Linux/macOS: canonical stable sort of at most512 eight-byte pairs uses its existing4096-byte stack scratch. The identity rail retains its512-byte limit and unsupported-shape refusal. Unicode17 canonical tables bound every nonstarter run by original input UTF8bytes; all2081 entries and algorithmic Hangul have prepared owner coverage.
- The controlled streaming rail orders more than512 pending pairs by canonical combining class using256 stack positions and a fallible exact temporary Vec. Equal-class order remains stable. Its Sort grant includes live decomposition/recomposition backing and is released after the sort buffer drops, including failure; iterator grants release after both iterator buffers drop.
- tinyvec rustc_1_57 feature supplies fallible native reserve; vendored minimum Rust is1.92 for the qualified native sort rail. Ordinary policy keeps original TinyVec push/sort behavior.
- Explicit native_scratch_v1 test target instruments allocations; test TLS is diagnostic only. No production TLS/global quota/allocator replacement.
- Workspace feature activation is source state, not behavior qualification. The long-text extension and its fixed-output/refusal regression sources have static checks only; Rust build/tests and runtime qualification are `NOT_RUN` in this audit.

## Original archive files

| Path | SHA256 |
| --- | --- |
| `.cargo_vcs_info.json` | `bf9b7dd295116a60116a858fbd160193e17aaa0db74bf8500e691a632015ec93` |
| `.github/workflows/rust.yml` | `b09d939e75311d17aff306731e7af696e18845345579cdfaec17486c04b95dc6` |
| `.gitignore` | `b2e67ce99bec6186eede939ac3867a2e6a508adbac43ebe021ba17e2fd0df49a` |
| `.travis.yml` | `a7b41b8c3a730ae331409fc96b45a0baded22923150a8f30cf2d6a9aa37fe30a` |
| `COPYRIGHT` | `23860c2a7b5d96b21569afedf033469bab9fe14a1b24a35068b8641c578ce24d` |
| `Cargo.lock` | `df20beff5ac96c98825b63712464083ca22c5532b7e52aa81c48e1ea3b3104ac` |
| `Cargo.toml` | `5b133b15dd596c7deaae9c09f6049e643c4371f23aff58cb7a72f2b13134b5c5` |
| `Cargo.toml.orig` | `39d19d80979b3c71708c1ee1191860aa77c52f3159629346bee75177ff74d838` |
| `LICENSE-APACHE` | `a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2` |
| `LICENSE-MIT` | `7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0` |
| `README.md` | `91ee30b7c76b5a7dc2cd19252f5dda5a12d2855ea8a218c1ce05e4cc32f62138` |
| `benches/bench.rs` | `2c756c3a084c2ffc740988373fbc01a0333f7e85692ef547cac5ac592bed3024` |
| `scripts/unicode.py` | `eec3b3fc1c56197eb270ce787501bb65b8bf07a67f7236512d1939776b4cfe3e` |
| `src/__test_api.rs` | `4ca9e8f342dee364d0ca82f80504051b9d7c88fb6726bbe2683898ed0b919dfa` |
| `src/decompose.rs` | `bd3b42065a5ccd95894d0ff0c24daac7eee90adebbbaf5bdf9347309d89bb035` |
| `src/lib.rs` | `f89636fd50f8b297cce678e7f8dbc532d1b43a7aca9bd120b06e3c295e70ba5d` |
| `src/lookups.rs` | `962f9909b32e02b8a2a05836135d9cd39bb1ce01f7c659de99cbd8a3a3c78574` |
| `src/normalize.rs` | `8a36365568a8a435d0da6496b1770a30d127f5b189443e6f1cf290296ded3564` |
| `src/perfect_hash.rs` | `400c84e2f467f61bd55d55d08672da6a9ad7a57c938ce5d0c701a6994b1b273b` |
| `src/quick_check.rs` | `eea209153e6d6b6ff242bc0df10678be2513a6440214bfdf1889321f9657d6d7` |
| `src/recompose.rs` | `30746c36c1f2929a7b89f2771d63fc857b5d57a87934c661d82c5649ff0cf391` |
| `src/replace.rs` | `0196e3e604203694f78a0b18852e30dc9f3c87637a2ae673aad4506dcf208322` |
| `src/stream_safe.rs` | `8b46d4246479addcdc5bc039d1d3bf4c594be3f8116fd6fcab65674e00d36af6` |
| `src/tables.rs` | `177d5f08019cc8e335444fcab61aabb7f6309f158f6ebbd7525c73c0e532ec44` |
| `src/test.rs` | `8af1c74989355970cc150075f0c8517bb061caeb46a456458483fe5ef661dc89` |
