//! Positional phrase matching over already validated, ascending posting lists.

use super::text::{CaseMode, MAX_TEXT_INSPECT_TERM_BYTES};
use quanta_index_core::index::{IndexError, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_lq_text_normalizer::{TextQueryError, TokenCursor, case_chars};

pub(super) const MAX_PHRASE_TERMS: usize = 64;

/// Re-run the canonical analyzer on the literal before any posting is opened.
/// The planner's ordered token sequence must be the exact result, including
/// repeated terms and the selected case mode.
pub(super) fn validate_tokens(
    raw: &str,
    tokens: &[String],
    case: CaseMode,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    if tokens.is_empty() || tokens.len() > MAX_PHRASE_TERMS {
        return Err(IndexError::InvalidInput(
            "native text phrase requires 1..=64 tokens".into(),
        ));
    }
    if raw.len() > MAX_TEXT_INSPECT_TERM_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    budget.charge(WorkUsage {
        bytes: u64::try_from(raw.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    // Match the existing text adapter's NFC output and normalization-work
    // envelopes. The raw query remains subject to the 4096-byte input bound.
    let mut cursor = TokenCursor::try_new_bounded(
        raw,
        16_777_216,
        67_108_864,
        |_units| budget.checkpoint(),
        |bytes| {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )
        },
    )
    .map_err(|error| {
        error
            .into_checkpoint()
            .map_or(IndexError::ResourceLimited, std::convert::identity)
    })?;
    let mut count = 0usize;
    while let Some(run) = cursor.next_run() {
        budget.checkpoint()?;
        if !run.is_indexable() {
            return Err(IndexError::InvalidInput(
                TextQueryError::TokenTooLong {
                    bytes: run.end.saturating_sub(run.start),
                    max: quanta_index_lq_text_normalizer::MAX_TOKEN_BYTES,
                }
                .to_string(),
            ));
        }
        let expected = tokens.get(count).ok_or_else(|| {
            IndexError::InvalidInput("native text phrase token count differs from literal".into())
        })?;
        let compared = run
            .text
            .len()
            .checked_add(expected.len())
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(compared).map_err(|_error| IndexError::ResourceLimited)?,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        if !case_chars(run.text, case).eq(expected.chars()) {
            return Err(IndexError::InvalidInput(
                "native text phrase tokens differ from literal".into(),
            ));
        }
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    if count != tokens.len() {
        return Err(IndexError::InvalidInput(
            "native text phrase token count differs from literal".into(),
        ));
    }
    budget.checkpoint()
}

/// Match consecutive token positions in query order.
///
/// Each input list comes from the text posting reader, which has already
/// validated that its positions are strictly ascending. Repeated query terms
/// may borrow the same list more than once. No position list is copied.
pub(super) fn matches_positions(
    positions: &[&[u32]],
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    budget.checkpoint()?;
    if positions.len() > MAX_PHRASE_TERMS {
        return Err(IndexError::InvalidInput(
            "native text phrase exceeds 64 terms".into(),
        ));
    }
    let Some((anchor_index, anchor)) = shortest_list(positions, budget)? else {
        return Ok(false);
    };
    if anchor.is_empty() {
        return Ok(false);
    }

    let anchor_offset =
        u32::try_from(anchor_index).map_err(|_error| IndexError::ResourceLimited)?;
    for &position in anchor {
        charge_position_probe(budget)?;
        let Some(start) = position.checked_sub(anchor_offset) else {
            continue;
        };
        let mut all_present = true;
        for (offset, list) in positions.iter().enumerate() {
            budget.charge(WorkUsage {
                mutations: 1,
                ..WorkUsage::default()
            })?;
            if offset == anchor_index {
                continue;
            }
            let offset = u32::try_from(offset).map_err(|_error| IndexError::ResourceLimited)?;
            let Some(target) = start.checked_add(offset) else {
                all_present = false;
                break;
            };
            if !contains_position(list, target, budget)? {
                all_present = false;
                break;
            }
        }
        if all_present {
            return Ok(true);
        }
    }
    Ok(false)
}

fn shortest_list<'a>(
    positions: &'a [&[u32]],
    budget: &mut WorkBudget,
) -> Result<Option<(usize, &'a [u32])>, IndexError> {
    let mut shortest = None;
    for (index, &list) in positions.iter().enumerate() {
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        if shortest.is_none_or(|(_prior_index, prior): (usize, &[u32])| list.len() < prior.len()) {
            shortest = Some((index, list));
        }
    }
    Ok(shortest)
}

fn contains_position(
    positions: &[u32],
    target: u32,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let mut low = 0usize;
    let mut high = positions.len();
    while low < high {
        let middle = low
            .checked_add(
                high.checked_sub(low)
                    .ok_or(IndexError::ResourceLimited)?
                    .div_euclid(2),
            )
            .ok_or(IndexError::ResourceLimited)?;
        charge_position_probe(budget)?;
        let position = positions.get(middle).ok_or_else(|| {
            IndexError::BackendViolation("phrase position search exceeded posting length".into())
        })?;
        match position.cmp(&target) {
            std::cmp::Ordering::Less => {
                low = middle.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            std::cmp::Ordering::Equal => return Ok(true),
            std::cmp::Ordering::Greater => high = middle,
        }
    }
    Ok(false)
}

fn charge_position_probe(budget: &mut WorkBudget) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(std::mem::size_of::<u32>())
            .map_err(|_error| IndexError::ResourceLimited)?,
        mutations: 1,
        ..WorkUsage::default()
    })
}

#[cfg(test)]
mod tests {
    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "phrase gold failed: {}",
                    stringify!($condition)
                )));
            }
        };
    }
    macro_rules! check_eq {
        ($actual:expr, $expected:expr $(,)?) => {{
            match (&$actual, &$expected) {
                (actual, expected) if actual == expected => {}
                (actual, expected) => return Err(IndexError::BackendViolation(format!(
                    "phrase gold differs: actual={actual:?}, expected={expected:?}"
                ))),
            }
        }};
        ($actual:expr, $expected:expr, $($message:tt)+) => {{
            match (&$actual, &$expected) {
                (actual, expected) if actual == expected => {}
                (actual, expected) => return Err(IndexError::BackendViolation(format!(
                    "{}: actual={actual:?}, expected={expected:?}", format!($($message)+)
                ))),
            }
        }};
    }
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{IndexError, WorkBudget, WorkLimits, WorkUsage};

    use super::{
        CaseMode, MAX_PHRASE_TERMS, MAX_TEXT_INSPECT_TERM_BYTES, matches_positions, validate_tokens,
    };

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: u64::MAX,
                writes: u64::MAX,
                bytes: u64::MAX,
                mutations: u64::MAX,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn fixed_positions_cover_order_repetition_gaps_and_boundaries() -> Result<(), IndexError> {
        let a = [2, 8];
        let b = [3, 9];
        let c = [4, 10];
        check!(matches_positions(&[&a, &b, &c], &mut budget()?)?);
        check!(matches_positions(
            &[&[0, 10], &[11], &[12]],
            &mut budget()?
        )?);
        check!(!matches_positions(&[&b, &a], &mut budget()?)?);
        check!(!matches_positions(&[&a, &c], &mut budget()?)?);
        check!(matches_positions(&[&a], &mut budget()?)?);
        check!(!matches_positions(&[], &mut budget()?)?);
        check!(!matches_positions(&[&a, &[]], &mut budget()?)?);

        let repeated = [5, 6, 7];
        check!(matches_positions(
            &[&repeated, &repeated, &repeated],
            &mut budget()?,
        )?);
        check!(!matches_positions(&[&[5], &[5]], &mut budget()?)?);
        check!(matches_positions(
            &[&[u32::MAX - 1], &[u32::MAX]],
            &mut budget()?,
        )?);
        check!(!matches_positions(
            &[&[u32::MAX], &[u32::MAX]],
            &mut budget()?,
        )?);
        check!(!matches_positions(&[&[0, 2], &[0]], &mut budget()?)?);
        Ok(())
    }

    #[test]
    fn exhaustive_small_universe_matches_brute_force_reference() -> Result<(), IndexError> {
        for width in 0..=3usize {
            let cases =
                16usize.pow(u32::try_from(width).map_err(|_error| IndexError::ResourceLimited)?);
            for case in 0..cases {
                let mut bits = case;
                let mut lists = Vec::new();
                for _ in 0..width {
                    let mask = bits & 0b1111;
                    bits >>= 4;
                    lists.push(
                        (0..4u32)
                            .filter(|position| mask & (1usize << position) != 0)
                            .collect::<Vec<_>>(),
                    );
                }
                let borrowed = lists.iter().map(Vec::as_slice).collect::<Vec<_>>();
                let expected = width > 0
                    && (0..4u32).any(|start| {
                        borrowed.iter().enumerate().all(|(offset, list)| {
                            match u32::try_from(offset) {
                                Ok(offset) => start
                                    .checked_add(offset)
                                    .is_some_and(|position| list.contains(&position)),
                                Err(_error) => false,
                            }
                        })
                    });
                check_eq!(
                    matches_positions(&borrowed, &mut budget()?)?,
                    expected,
                    "width={width}, case={case}",
                );
            }
        }
        Ok(())
    }

    #[test]
    fn width_cancellation_and_original_budget_are_enforced() -> Result<(), IndexError> {
        let one_position = [0u32];
        let repeated = [one_position.as_slice(); MAX_PHRASE_TERMS + 1];
        check!(matches!(
            matches_positions(&repeated, &mut budget()?),
            Err(IndexError::InvalidInput(_))
        ));

        let mut one = budget()?;
        check!(matches_positions(&[&[7]], &mut one)?);
        check_eq!(
            one.usage(),
            WorkUsage {
                bytes: 4,
                mutations: 3,
                ..WorkUsage::default()
            }
        );
        let mut limited = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 3,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check_eq!(
            matches_positions(&[&[7]], &mut limited),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(limited.usage().bytes, 0);

        let mut cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        check_eq!(
            matches_positions(&[&[7]], &mut cancelled),
            Err(IndexError::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn literal_validation_binds_exact_analyzer_order_and_case() -> Result<(), IndexError> {
        validate_tokens(
            "Blue, BLUE whale",
            &["blue".into(), "blue".into(), "whale".into()],
            CaseMode::Folded,
            &mut budget()?,
        )?;
        validate_tokens(
            "cafe\u{301} İ",
            &["café".into(), "i\u{307}".into()],
            CaseMode::Folded,
            &mut budget()?,
        )?;
        validate_tokens(
            "Blue whale",
            &["Blue".into(), "whale".into()],
            CaseMode::Sensitive,
            &mut budget()?,
        )?;
        for (raw, expected) in [
            ("Blue whale", vec!["whale".into(), "blue".into()]),
            ("Blue whale", vec!["blue".into()]),
            ("Blue whale", vec!["blue".into(), "blue".into()]),
            (
                "Blue whale",
                vec!["blue".into(), "whale".into(), "extra".into()],
            ),
            ("😄 !", vec!["emoji".into()]),
        ] {
            check!(matches!(
                validate_tokens(raw, &expected, CaseMode::Folded, &mut budget()?),
                Err(IndexError::InvalidInput(_))
            ));
        }
        check!(matches!(
            validate_tokens(
                "Blue whale",
                &["blue".into(), "whale".into()],
                CaseMode::Sensitive,
                &mut budget()?,
            ),
            Err(IndexError::InvalidInput(_))
        ));
        Ok(())
    }

    #[test]
    fn literal_validation_refuses_limits_and_cancellation() -> Result<(), IndexError> {
        let repeated = vec!["x".to_string(); MAX_PHRASE_TERMS + 1];
        check!(matches!(
            validate_tokens("x", &repeated, CaseMode::Folded, &mut budget()?),
            Err(IndexError::InvalidInput(_))
        ));
        check!(matches!(
            validate_tokens("x", &[], CaseMode::Folded, &mut budget()?),
            Err(IndexError::InvalidInput(_))
        ));
        let long = "x".repeat(MAX_TEXT_INSPECT_TERM_BYTES + 1);
        check_eq!(
            validate_tokens(&long, &["x".into()], CaseMode::Folded, &mut budget()?),
            Err(IndexError::ResourceLimited)
        );
        let token_too_long = "x".repeat(quanta_index_lq_text_normalizer::MAX_TOKEN_BYTES + 1);
        check!(matches!(
            validate_tokens(
                &token_too_long,
                std::slice::from_ref(&token_too_long),
                CaseMode::Folded,
                &mut budget()?,
            ),
            Err(IndexError::InvalidInput(_))
        ));
        let mut cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        check_eq!(
            validate_tokens("x", &["x".into()], CaseMode::Folded, &mut cancelled),
            Err(IndexError::Cancelled)
        );
        Ok(())
    }
}
