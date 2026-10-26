//! Request-bound validation for saved-source action replies.

use super::NativeUdsError;
use quanta_index_contract::native_protocol::{
    NativeStubBinding, NativeStubCoordinateUnit, NativeStubEdit, NativeStubItem, NativeStubReply,
    NativeStubRequest,
};

const MAX_SOURCE_BYTES: usize = 1 << 20;
const MAX_EDITS: usize = 64;

fn binding_valid(binding: &NativeStubBinding) -> bool {
    binding.selected.workspace == binding.document.session.workspace
        && binding.selected.epoch != 0
        && binding.revision != 0
        && binding.document.session.epoch != 0
        && binding.document.session.publisher.0 != [0; 32]
        && binding.source.0.0 != [0; 32]
        && !binding.language.is_empty()
        && binding.language.len() <= 64
}

fn items_valid(items: &[NativeStubItem], exact_source: &str) -> bool {
    !items.is_empty()
        && items.len() <= 128
        && items.iter().enumerate().all(|(at, item)| {
            item.stable_id.0 != [0; 32]
                && item.ast_digest.0 != [0; 32]
                && item.start < item.end
                && usize::try_from(item.end).is_ok_and(|end| end <= exact_source.len())
                && usize::try_from(item.start)
                    .is_ok_and(|start| exact_source.is_char_boundary(start))
                && usize::try_from(item.end).is_ok_and(|end| exact_source.is_char_boundary(end))
                && !item.label.is_empty()
                && item.label.len() <= 256
                && !items
                    .iter()
                    .take(at)
                    .any(|prior| prior.stable_id == item.stable_id)
        })
}

#[derive(Clone, Copy, Default)]
struct ConvertedEdit {
    request_index: usize,
    start: u32,
    end: u32,
}

fn converted_edits(
    items: &[NativeStubItem],
    selected_id: quanta_index_contract::index::Digest,
    edits: &[NativeStubEdit],
    unit: NativeStubCoordinateUnit,
    exact_source: &str,
) -> Option<([ConvertedEdit; MAX_EDITS], usize)> {
    if edits.is_empty() || edits.len() > MAX_EDITS {
        return None;
    }
    let mut converted = [ConvertedEdit::default(); MAX_EDITS];
    let mut total_replacement_bytes = 0usize;
    let mut selected_target_seen = false;
    for (index, edit) in edits.iter().enumerate() {
        let item = items.iter().find(|item| item.stable_id == edit.target_id)?;
        if item.ast_digest != edit.expected_ast || edit.replacement.len() > 64 << 10 {
            return None;
        }
        let (start, end) = edit.range.to_byte_range(exact_source, unit)?;
        if start < item.start || end > item.end {
            return None;
        }
        total_replacement_bytes = total_replacement_bytes.checked_add(edit.replacement.len())?;
        if total_replacement_bytes > 256 << 10 {
            return None;
        }
        selected_target_seen |= edit.target_id == selected_id;
        *converted.get_mut(index)? = ConvertedEdit {
            request_index: index,
            start,
            end,
        };
    }
    if !selected_target_seen {
        return None;
    }
    let used = converted.get_mut(..edits.len())?;
    used.sort_unstable_by_key(|edit| (edit.start, edit.end));
    if used.iter().zip(used.iter().skip(1)).any(|(left, right)| {
        let next_start = right.start;
        left.start >= next_start || left.end > next_start
    }) {
        return None;
    }
    Some((converted, edits.len()))
}

pub(super) fn validate_request(
    request: &NativeStubRequest,
    exact_source: &str,
) -> Result<(), NativeUdsError> {
    let valid = exact_source.len() <= MAX_SOURCE_BYTES
        && match request {
            NativeStubRequest::Certify { binding, items } => {
                binding_valid(binding) && items_valid(items, exact_source)
            }
            NativeStubRequest::Preflight {
                binding,
                items,
                selected_id,
                edits,
                unit,
            } => {
                binding_valid(binding)
                    && items_valid(items, exact_source)
                    && items.iter().any(|item| item.stable_id == *selected_id)
                    && converted_edits(items, *selected_id, edits, *unit, exact_source).is_some()
            }
        };
    if valid {
        Ok(())
    } else {
        Err(NativeUdsError::Protocol(
            "Stub request exceeds source/action bounds".into(),
        ))
    }
}

pub(super) fn validate_reply(
    request: &NativeStubRequest,
    reply: &NativeStubReply,
    exact_source: &str,
) -> Result<(), NativeUdsError> {
    let valid = match (request, reply) {
        (
            NativeStubRequest::Certify { binding, items },
            NativeStubReply::Certified {
                binding: actual,
                items: output,
                ..
            },
        ) => {
            binding == actual
                && output.len() == items.len()
                && items_valid(output, exact_source)
                && output
                    .iter()
                    .zip(output.iter().skip(1))
                    .all(|(left, right)| left.stable_id < right.stable_id)
                && output
                    .iter()
                    .all(|item| items.iter().any(|original| original == item))
        }
        (
            NativeStubRequest::Preflight {
                binding,
                items,
                selected_id,
                edits,
                unit,
            },
            NativeStubReply::Prepared {
                binding: actual,
                selected_id: actual_id,
                edits: output,
            },
        ) => {
            let expected = converted_edits(items, *selected_id, edits, *unit, exact_source);
            binding == actual
                && selected_id == actual_id
                && expected.as_ref().is_some_and(|(expected, count)| {
                    *count == output.len()
                        && expected.get(..*count).is_some_and(|expected| {
                            expected.iter().zip(output).all(|(source, actual)| {
                                edits.get(source.request_index).is_some_and(|original| {
                                    actual.target_id == original.target_id
                                        && actual.start == source.start
                                        && actual.end == source.end
                                        && actual.replacement == original.replacement.as_bytes()
                                })
                            })
                        })
                })
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(NativeUdsError::Protocol(
            "Stub reply differs from source/action request".into(),
        ))
    }
}
