//! Regex engines for short metadata fields such as repository ids.
//!
//! A document engine is admitted at its configured worst case because its
//! lazy DFA cache grows with the documents it scans. A field engine only
//! answers whether one short value matches, so it is a Thompson NFA searched
//! by a `PikeVM` with no capture slots, and every byte it can retain is known
//! after construction:
//!
//! - the NFA, as reported by `NFA::memory_usage`; construction is capped by
//!   [`FIELD_NFA_SIZE_LIMIT_BYTES`] through `nfa_size_limit`;
//! - the `PikeVM` cache's two active-state sets, which `PikeVM::create_cache`
//!   allocates at their final size (one sparse-set entry per NFA state);
//! - the epsilon-closure stack. Each closure explores a state at most once,
//!   and only a union pushes frames (one per alternate after the first), so
//!   the stack never holds more than one frame plus one per such alternate.
//!
//! The match set is the same language as the document engine's: both are
//! built from the same validated execution HIR with leftmost-first
//! semantics, and a field match only asks whether any match exists.

use core::mem::size_of;
use std::sync::{Mutex, PoisonError};

use regex_automata::nfa::thompson::{self, State, WhichCaptures, pikevm};
use regex_automata::{Input, MatchKind};

use crate::errors::{LimitDimension, RegexError, RegexErrorCode};
use crate::executor::RegexCompilationPlan;

/// NFA heap ceiling for one field engine.
pub const FIELD_NFA_SIZE_LIMIT_BYTES: usize = 1 << 20;

/// One epsilon-closure frame: a state id or a capture slot with an optional
/// offset, plus the enum tag and padding. Three words bound either variant.
const CLOSURE_FRAME_BYTES: usize = 3 * size_of::<usize>();

/// A compiled field regex whose retained heap is known exactly.
pub struct FieldRegex {
    vm: pikevm::PikeVM,
    cache: Mutex<pikevm::Cache>,
    retained_bytes: usize,
}

impl FieldRegex {
    /// Peak heap of building one field engine: its NFA builder, capped by
    /// [`FIELD_NFA_SIZE_LIMIT_BYTES`], and the NFA it emits, never larger
    /// than the builder, are held at once.
    pub const CONSTRUCTION_BYTES: usize = 2 * FIELD_NFA_SIZE_LIMIT_BYTES;

    /// Compile a validated plan into a field engine.
    ///
    /// Errors: [`RegexErrorCode::PlanLimitExceeded`] when the NFA exceeds
    /// [`FIELD_NFA_SIZE_LIMIT_BYTES`], or [`RegexErrorCode::ExecutionInternal`]
    /// for any other construction failure.
    pub fn compile_prepared(plan: &RegexCompilationPlan) -> Result<Self, RegexError> {
        let nfa = thompson::Compiler::new()
            .configure(
                thompson::Config::new()
                    .nfa_size_limit(Some(FIELD_NFA_SIZE_LIMIT_BYTES))
                    .utf8(false)
                    .which_captures(WhichCaptures::None),
            )
            .build_from_hir(plan.execution_hir())
            .map_err(|error| {
                error.size_limit().map_or_else(
                    || {
                        RegexError::new(
                            RegexErrorCode::ExecutionInternal,
                            format!("validated field regex failed NFA construction: {error}"),
                        )
                    },
                    |limit| {
                        RegexError::plan_limit(
                            LimitDimension::CompiledBytes,
                            format!("field regex exceeds NFA byte ceiling {limit}"),
                        )
                    },
                )
            })?;
        let closure_frames = nfa.states().iter().fold(1_usize, |frames, state| {
            let pushed = match state {
                State::Union { alternates } => alternates.len().saturating_sub(1),
                State::BinaryUnion { .. } => 1,
                State::ByteRange { .. }
                | State::Sparse(_)
                | State::Dense(_)
                | State::Look { .. }
                | State::Capture { .. }
                | State::Fail
                | State::Match { .. } => 0,
            };
            frames.saturating_add(pushed)
        });
        let vm = pikevm::PikeVM::builder()
            .configure(pikevm::PikeVM::config().match_kind(MatchKind::LeftmostFirst))
            .build_from_nfa(nfa)
            .map_err(|error| {
                RegexError::new(
                    RegexErrorCode::ExecutionInternal,
                    format!("validated field regex failed PikeVM construction: {error}"),
                )
            })?;
        let cache = vm.create_cache();
        let retained_bytes = vm
            .get_nfa()
            .memory_usage()
            .checked_add(cache.memory_usage())
            .and_then(|bytes| {
                closure_frames
                    .checked_mul(CLOSURE_FRAME_BYTES)
                    .and_then(|stack| bytes.checked_add(stack))
            })
            .ok_or_else(|| {
                RegexError::plan_limit(
                    LimitDimension::CompiledBytes,
                    "field regex retained bytes overflow",
                )
            })?;
        Ok(Self {
            vm,
            cache: Mutex::new(cache),
            retained_bytes,
        })
    }

    /// Whether the field value contains a match.
    #[must_use]
    pub fn is_match(&self, field: &[u8]) -> bool {
        // The `PikeVM` resets its cache at every search, so a cache left by a
        // panicking holder is still valid scratch.
        let mut cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => PoisonError::into_inner(poisoned),
        };
        self.vm.is_match(&mut cache, Input::new(field))
    }

    /// Every heap byte this engine can retain: its NFA, its `PikeVM` cache and
    /// the largest epsilon-closure stack a search can build.
    #[must_use]
    pub const fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning field engine tests propagate setup errors and assert fixed oracles"
)]
mod tests {
    use super::*;
    use crate::RegexExecutor;

    fn field(pattern: &str) -> Result<FieldRegex, RegexError> {
        FieldRegex::compile_prepared(&RegexExecutor::prepare(pattern)?)
    }

    /// Field and document engines answer the same match question.
    #[test]
    fn field_engine_matches_like_the_document_engine() -> Result<(), RegexError> {
        let values = [
            "corp-a",
            "corp-b",
            "org/alpha",
            "répo/../%",
            "a",
            "CORP-A",
            "x corp-a y",
            // Authoritative paths and file names.
            "src/main.rs",
            "src/nested/main.rs",
            "main.rs",
            "tests/ü/日本.rs",
            "a/b.c/d",
        ];
        for pattern in [
            "corp-a",
            "^corp-[a-z]+$",
            "a$",
            "c.rp-[a-z]",
            r"\w+",
            r"(?i)corp-\w+",
            r"\bcorp\b",
            r"[\p{L}\d]+-\d+",
            r"^.*$",
            "é",
            "^src/",
            r"[.]rs$",
            "^main",
            r"(?i)\w+/MAIN",
            r"^[^/]+$",
            r"(?:^|/)日本[.]rs$",
            r"\.c/",
        ] {
            let field = field(pattern)?;
            let document = RegexExecutor::compile(pattern)?;
            for value in values {
                assert_eq!(
                    field.is_match(value.as_bytes()),
                    document.verify(value.as_bytes()),
                    "{pattern:?} on {value:?}"
                );
            }
        }
        Ok(())
    }

    /// A whole-value field engine answers like a whole-value document engine.
    #[test]
    fn full_match_field_engine_matches_like_the_document_engine() -> Result<(), RegexError> {
        for pattern in [
            r"src/.*[.]rs",
            "src/lib.rs",
            r"\w+[.]rs",
            "main.rs",
            "(?i)SRC/.*",
            "a|ab",
        ] {
            let field =
                FieldRegex::compile_prepared(&RegexExecutor::prepare(pattern)?.into_full_match()?)?;
            let document = RegexExecutor::compile_prepared(
                RegexExecutor::prepare(pattern)?.into_full_match()?,
            )?;
            for value in [
                "src/lib.rs",
                "src/a/b.rs",
                "x/src/lib.rs",
                "src/lib.rs.bak",
                "lib.rs",
                "main.rs",
                "domain.rs",
                "SRC/LIB.RS",
                "ab",
                "abc",
                "",
            ] {
                assert_eq!(
                    field.is_match(value.as_bytes()),
                    document.verify(value.as_bytes()),
                    "{pattern:?} on {value:?}"
                );
            }
        }
        Ok(())
    }

    /// A fixed xorshift stream.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            let len = u64::try_from(items.len()).map_or(1, |len| len.max(1));
            usize::try_from(self.next().checked_rem(len).unwrap_or_default())
                .map_or(None, |slot| items.get(slot).copied())
                .unwrap_or_default()
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next().checked_rem(bound.max(1)).unwrap_or_default()
        }
    }

    /// Random small patterns over path-like atoms, with repetition, anchors,
    /// alternation and case folding.
    fn random_pattern(rng: &mut Rng) -> String {
        const ATOMS: [&str; 14] = [
            "a", "b", "/", "[.]", "[ab]", "[^/]", r"\w", r"\d", "é", "(?:a|b/)", "x", ".", "_",
            r"\b",
        ];
        const REPEATS: [&str; 6] = ["", "", "*", "+", "?", "{1,2}"];
        let mut pattern = String::new();
        if rng.below(4) == 0 {
            pattern.push_str("(?i)");
        }
        if rng.below(3) == 0 {
            pattern.push('^');
        }
        for _ in 0..=rng.below(4) {
            let atom = rng.pick(&ATOMS);
            pattern.push_str(atom);
            if atom != r"\b" {
                pattern.push_str(rng.pick(&REPEATS));
            }
        }
        if rng.below(5) == 0 {
            pattern.push('|');
            pattern.push_str(rng.pick(&ATOMS));
        }
        if rng.below(3) == 0 {
            pattern.push('$');
        }
        pattern
    }

    fn random_value(rng: &mut Rng) -> String {
        const UNITS: [&str; 12] = ["a", "b", "A", "B", "/", ".", "1", "é", "É", "x", "_", " "];
        (0..rng.below(10)).map(|_| rng.pick(&UNITS)).collect()
    }

    /// On random patterns and values, a field engine answers exactly like
    /// the document engine built from the same plan, unanchored and as a
    /// whole-value match.
    #[test]
    fn field_engine_equals_document_engine_on_random_values() -> Result<(), RegexError> {
        let mut rng = Rng(0xf1e1_d0c5_0bad_5eed);
        let mut matched = 0_usize;
        let mut checked = 0_usize;
        for _ in 0..300 {
            let pattern = random_pattern(&mut rng);
            let values: Vec<String> = (0..12).map(|_| random_value(&mut rng)).collect();
            for full in [false, true] {
                let plan = |pattern: &str| {
                    let plan = RegexExecutor::prepare(pattern)?;
                    if full {
                        plan.into_full_match()
                    } else {
                        Ok(plan)
                    }
                };
                let field = FieldRegex::compile_prepared(&plan(&pattern)?)?;
                let document = RegexExecutor::compile_prepared(plan(&pattern)?)?;
                for value in &values {
                    let answer = field.is_match(value.as_bytes());
                    assert_eq!(
                        answer,
                        document.verify(value.as_bytes()),
                        "{pattern:?} full={full} on {value:?}"
                    );
                    matched = matched.saturating_add(usize::from(answer));
                    checked = checked.saturating_add(1);
                }
            }
        }
        // Both answers must be well represented, or the oracle is vacuous.
        assert!(
            matched.saturating_mul(10) > checked
                && matched.saturating_mul(10) < checked.saturating_mul(9),
            "{matched} of {checked} matched"
        );
        Ok(())
    }

    /// Retained bytes cover the NFA and the searches' scratch, and stay far
    /// below a document engine's configured ceiling for ordinary filters.
    #[test]
    fn field_engine_retained_bytes_cover_nfa_and_scratch() -> Result<(), RegexError> {
        for pattern in [
            "corp-a",
            "^corp-[a-z]+$",
            r"(?i)corp-\w+",
            r"[\p{L}\d]+-\d+",
        ] {
            let field = field(pattern)?;
            let nfa = field.vm.get_nfa().memory_usage();
            let mut cache = field.vm.create_cache();
            let _found = field.vm.is_match(&mut cache, Input::new(b"corp-a/x-1"));
            assert!(
                field.retained_bytes() >= nfa + cache.memory_usage(),
                "{pattern:?}"
            );
            assert!(field.retained_bytes() < (1 << 20), "{pattern:?}");
        }
        // This range expands into enough UTF-8 states to pass the planning
        // estimate and exceed the field NFA ceiling.
        let plan = RegexExecutor::prepare(r"[\x{80}-\x{10FFFF}]{8000}")?;
        assert!(plan.estimated_states() <= crate::MAX_NFA_STATES);
        let refused = FieldRegex::compile_prepared(&plan);
        assert!(
            matches!(&refused, Err(error) if error.code == RegexErrorCode::PlanLimitExceeded
                && error.dimension == Some(LimitDimension::CompiledBytes)),
            "an NFA over the field ceiling must be a typed limit"
        );
        Ok(())
    }
}
