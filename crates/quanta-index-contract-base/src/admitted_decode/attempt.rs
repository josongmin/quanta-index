//! One bounded parser attempt and its field scopes. Metadata only: this state
//! cannot issue memory grants, authenticate a Source, or create another parser.

use super::DecodeBackingRefusalV1;

/// Units match the schema field: UTF-8 bytes, bytes, or sequence items.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeFieldKindV1 {
    Text,
    Bytes,
    Sequence,
}

/// An owner-issued LIFO marker; it is not an allocation authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeFieldTokenV1(u64);

#[derive(Clone, Copy, Debug)]
struct FieldFrame {
    kind: DecodeFieldKindV1,
    maximum: usize,
    token: DecodeFieldTokenV1,
    sequence_header_seen: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Created,
    Preflighting,
    Ready,
    Reading,
    Complete,
    Aborted,
}

// The existing finite Ciborium parser has a 256-level recursion limit. Field
// scopes need no unadmitted heap vector and cannot grow past that same bound.
const MAX_FIELD_DEPTH: usize = 256;

/// Nonreusable attempt state, held by the same owner as parser/visitor grants.
/// Owners must also bind access to the thread/role that began the parser.
#[derive(Debug)]
pub struct DecodeAttemptStateV1 {
    phase: Phase,
    fields: [Option<FieldFrame>; MAX_FIELD_DEPTH],
    depth: usize,
    sequence: u64,
    owned_input: bool,
}

impl Default for DecodeAttemptStateV1 {
    fn default() -> Self {
        Self {
            phase: Phase::Created,
            fields: [None; MAX_FIELD_DEPTH],
            depth: 0,
            sequence: 0,
            owned_input: false,
        }
    }
}

impl DecodeAttemptStateV1 {
    pub fn begin_preflight_v1(&mut self) -> Result<(), DecodeBackingRefusalV1> {
        if self.phase != Phase::Created || self.depth != 0 || self.owned_input {
            return self.refuse();
        }
        self.phase = Phase::Preflighting;
        Ok(())
    }

    pub fn complete_preflight_v1(&mut self) -> Result<(), DecodeBackingRefusalV1> {
        if self.phase != Phase::Preflighting || self.depth != 0 || self.owned_input {
            return self.refuse();
        }
        self.phase = Phase::Ready;
        Ok(())
    }

    #[must_use]
    pub fn is_preflight_complete_v1(&self) -> bool {
        self.phase == Phase::Ready
    }

    pub fn begin_decode_v1(&mut self) -> Result<(), DecodeBackingRefusalV1> {
        if !matches!(self.phase, Phase::Created | Phase::Ready)
            || self.depth != 0
            || self.owned_input
        {
            return self.refuse();
        }
        self.phase = Phase::Reading;
        Ok(())
    }

    pub fn complete_decode_v1(&mut self) -> Result<(), DecodeBackingRefusalV1> {
        self.require_reading()?;
        if self.depth != 0 || self.owned_input {
            return self.refuse();
        }
        self.phase = Phase::Complete;
        Ok(())
    }

    /// Sticky terminal state; never refunds or clears retained failure DATA.
    pub fn abort_v1(&mut self) {
        self.phase = Phase::Aborted;
    }

    #[must_use]
    pub fn is_reading_v1(&self) -> bool {
        self.phase == Phase::Reading
    }

    pub fn enter_field_v1(
        &mut self,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        self.require_reading()?;
        if self.owned_input || self.depth >= self.fields.len() {
            return self.refuse();
        }
        let Some(sequence) = self.sequence.checked_add(1) else {
            return self.refuse();
        };
        let Some(next_depth) = self.depth.checked_add(1) else {
            return self.refuse();
        };
        let token = DecodeFieldTokenV1(sequence);
        let Some(slot) = self.fields.get_mut(self.depth) else {
            return self.refuse();
        };
        *slot = Some(FieldFrame {
            kind,
            maximum,
            token,
            sequence_header_seen: false,
        });
        self.depth = next_depth;
        self.sequence = sequence;
        Ok(token)
    }

    pub fn leave_field_v1(
        &mut self,
        token: DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.require_reading()?;
        let Some(index) = self.depth.checked_sub(1) else {
            return self.refuse();
        };
        if self.owned_input
            || self
                .fields
                .get(index)
                .and_then(Option::as_ref)
                .is_none_or(|frame| frame.token != token)
        {
            return self.refuse();
        }
        let Some(slot) = self.fields.get_mut(index) else {
            return self.refuse();
        };
        *slot = None;
        self.depth = index;
        Ok(())
    }

    /// Distinct nested sequences have independent item counts. Their enclosing
    /// field is restored when the child leaves. Text/byte policy scopes tighten
    /// the bound on the same scalar parser backing.
    #[must_use]
    pub fn limit_v1(&self, kind: DecodeFieldKindV1, global: usize) -> usize {
        if self.depth > self.fields.len() {
            return 0;
        }
        if kind == DecodeFieldKindV1::Sequence {
            return self
                .fields
                .iter()
                .take(self.depth)
                .flatten()
                .rev()
                .find(|frame| frame.kind == kind)
                .map_or(global, |frame| global.min(frame.maximum));
        }
        self.fields
            .iter()
            .take(self.depth)
            .flatten()
            .filter(|frame| frame.kind == kind)
            .fold(global, |limit, frame| limit.min(frame.maximum))
    }

    pub fn check_length_v1(
        &mut self,
        kind: DecodeFieldKindV1,
        length: Option<usize>,
        global: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.require_reading()?;
        if self.owned_input
            || self.depth > self.fields.len()
            || length.is_some_and(|len| len > self.limit_v1(kind, global))
        {
            return self.refuse();
        }
        Ok(())
    }

    /// A byte field may use an integer array; its declared count is byte count.
    pub fn check_sequence_length_v1(
        &mut self,
        length: Option<usize>,
        global: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.check_length_v1(DecodeFieldKindV1::Sequence, length, global)?;
        if self
            .fields
            .iter()
            .take(self.depth)
            .flatten()
            .any(|frame| frame.kind == DecodeFieldKindV1::Bytes)
        {
            self.check_length_v1(DecodeFieldKindV1::Bytes, length, global)?;
        }
        Ok(())
    }

    /// Only the current field's first container header owns its item bound.
    /// Implicit nested fixed arrays (for example digest bytes) are different
    /// values; they must not inherit the outer Vec's item-count limit.
    pub fn check_sequence_header_v1(
        &mut self,
        length: Option<usize>,
        global: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.require_reading()?;
        if self.owned_input || self.depth > self.fields.len() {
            return self.refuse();
        }
        let index = self.fields.iter().take(self.depth).rposition(|frame| {
            frame.is_some_and(|frame| {
                matches!(
                    frame.kind,
                    DecodeFieldKindV1::Sequence | DecodeFieldKindV1::Bytes
                )
            })
        });
        let limit = index
            .and_then(|index| self.fields.get(index).copied().flatten())
            .filter(|frame| !frame.sequence_header_seen)
            .map_or(global, |frame| global.min(frame.maximum));
        if length.is_some_and(|length| length > limit) {
            return self.refuse();
        }
        if let Some(index) = index {
            let Some(frame) = self.fields.get_mut(index).and_then(Option::as_mut) else {
                return self.refuse();
            };
            frame.sequence_header_seen = true;
        }
        Ok(())
    }

    pub fn begin_owned_input_v1(&mut self) -> Result<(), DecodeBackingRefusalV1> {
        self.require_reading()?;
        if self.owned_input {
            return self.refuse();
        }
        self.owned_input = true;
        Ok(())
    }

    /// True only while the parser's owned input is checked out for validation.
    #[must_use]
    pub fn has_owned_input_v1(&self) -> bool {
        self.owned_input
    }

    /// Return failed input to external DATA before calling this with false.
    pub fn finish_owned_input_v1(&mut self, success: bool) {
        self.owned_input = false;
        if !success {
            self.abort_v1();
        }
    }

    fn require_reading(&mut self) -> Result<(), DecodeBackingRefusalV1> {
        if self.phase != Phase::Reading {
            return self.refuse();
        }
        Ok(())
    }

    fn refuse<T>(&mut self) -> Result<T, DecodeBackingRefusalV1> {
        self.abort_v1();
        Err(DecodeBackingRefusalV1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_excludes_parser_reentry_and_only_ready_enters_reading() {
        let mut state = DecodeAttemptStateV1::default();
        state.begin_preflight_v1().unwrap();
        assert_eq!(state.begin_decode_v1(), Err(DecodeBackingRefusalV1));
        assert_eq!(state.complete_preflight_v1(), Err(DecodeBackingRefusalV1));
        let mut state = DecodeAttemptStateV1::default();
        state.begin_preflight_v1().unwrap();
        state.complete_preflight_v1().unwrap();
        state.begin_decode_v1().unwrap();
        state.complete_decode_v1().unwrap();
        assert_eq!(state.begin_preflight_v1(), Err(DecodeBackingRefusalV1));
    }

    #[test]
    fn nested_caps_do_not_widen_and_restore_lifo() {
        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let outer = state.enter_field_v1(DecodeFieldKindV1::Text, 4).unwrap();
        let inner = state.enter_field_v1(DecodeFieldKindV1::Text, 40).unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Text, 400), 4);
        state.leave_field_v1(inner).unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Text, 400), 4);
        state.leave_field_v1(outer).unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Text, 400), 400);
        state.complete_decode_v1().unwrap();
        assert_eq!(state.begin_decode_v1(), Err(DecodeBackingRefusalV1));
    }

    #[test]
    fn bad_order_and_owned_input_reentry_poison_without_reset() {
        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let outer = state.enter_field_v1(DecodeFieldKindV1::Bytes, 2).unwrap();
        let _inner = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 20)
            .unwrap();
        assert_eq!(state.leave_field_v1(outer), Err(DecodeBackingRefusalV1));
        assert_eq!(state.complete_decode_v1(), Err(DecodeBackingRefusalV1));
        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        state.begin_owned_input_v1().unwrap();
        assert_eq!(state.begin_owned_input_v1(), Err(DecodeBackingRefusalV1));
        state.finish_owned_input_v1(true);
        assert_eq!(state.begin_decode_v1(), Err(DecodeBackingRefusalV1));
    }

    #[test]
    fn byte_array_header_uses_byte_cap_before_elements() {
        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let _field = state.enter_field_v1(DecodeFieldKindV1::Bytes, 2).unwrap();
        assert_eq!(
            state.check_sequence_header_v1(Some(3), 400),
            Err(DecodeBackingRefusalV1)
        );
        assert!(!state.is_reading_v1());
    }

    #[test]
    fn nested_sequences_use_their_own_item_bounds_then_restore_the_parent() {
        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let outer = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 1)
            .unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Sequence, 400), 1);
        state.check_sequence_header_v1(Some(1), 400).unwrap();
        let inner = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 3)
            .unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Sequence, 400), 3);
        state.check_sequence_header_v1(Some(3), 400).unwrap();
        state.leave_field_v1(inner).unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Sequence, 400), 1);
        let sibling = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 2)
            .unwrap();
        state.check_sequence_header_v1(Some(2), 400).unwrap();
        state.leave_field_v1(sibling).unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Sequence, 400), 1);
        state
            .check_length_v1(DecodeFieldKindV1::Sequence, Some(1), 400)
            .unwrap();
        state.leave_field_v1(outer).unwrap();
        state.complete_decode_v1().unwrap();

        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let outer = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 1)
            .unwrap();
        let inner = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 3)
            .unwrap();
        state.leave_field_v1(inner).unwrap();
        assert_eq!(
            state.check_sequence_length_v1(Some(2), 400),
            Err(DecodeBackingRefusalV1)
        );
        assert!(!state.is_reading_v1());
        assert_eq!(state.leave_field_v1(outer), Err(DecodeBackingRefusalV1));
    }

    #[test]
    fn implicit_nested_array_header_uses_global_cap_but_parent_growth_stays_bounded() {
        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let outer = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 1)
            .unwrap();
        state.check_sequence_header_v1(Some(1), 400).unwrap();
        state.check_sequence_header_v1(Some(3), 400).unwrap();
        assert_eq!(state.limit_v1(DecodeFieldKindV1::Sequence, 400), 1);
        assert_eq!(
            state.check_length_v1(DecodeFieldKindV1::Sequence, Some(2), 400),
            Err(DecodeBackingRefusalV1)
        );
        assert!(!state.is_reading_v1());
        assert_eq!(state.leave_field_v1(outer), Err(DecodeBackingRefusalV1));

        let mut state = DecodeAttemptStateV1::default();
        state.begin_decode_v1().unwrap();
        let outer = state
            .enter_field_v1(DecodeFieldKindV1::Sequence, 1)
            .unwrap();
        assert_eq!(
            state.check_sequence_header_v1(Some(2), 400),
            Err(DecodeBackingRefusalV1)
        );
        assert!(!state.is_reading_v1());
        assert_eq!(state.leave_field_v1(outer), Err(DecodeBackingRefusalV1));
    }
}
