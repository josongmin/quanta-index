//! Immutable, bounded membership blocks for the native byte-literal index.
//!
//! Directory map keys are the last `(gram, owner)` key in each block. Values
//! are dependency-free ObjectRefs; the object port authenticates block bytes.
//! This module does not grant publication authority or replace transition seal.

use quanta_index_core::index::{
    IndexError, MapRoot, MapValue, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectRef,
    StoredObject, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

const MAGIC: [u8; 4] = *b"QILB";
const VERSION_V1: u8 = 1;
const VERSION_V2: u8 = 2;
const V1_HEADER_BYTES: usize = 7;
const V2_HEADER_BYTES: usize = 13;
const RESTART_INTERVAL: usize = 16;
const RESTART_BYTES: usize = 2;
pub(super) const KEY_BYTES: usize = 35;
pub(super) const MAX_BLOCK_BYTES: usize = 32 * 1024;
const MAX_V1_KEYS: usize = (MAX_BLOCK_BYTES - V1_HEADER_BYTES) / KEY_BYTES;
const MAX_RESTARTS: usize = MAX_BLOCK_BYTES / (KEY_BYTES + 1);

pub(super) fn literal_key(gram: [u8; 3], owner: [u8; 32]) -> [u8; KEY_BYTES] {
    let mut key = [0; KEY_BYTES];
    if let Some(prefix) = key.get_mut(..3) {
        prefix.copy_from_slice(&gram);
    }
    if let Some(suffix) = key.get_mut(3..) {
        suffix.copy_from_slice(&owner);
    }
    key
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PushResult {
    Added,
    Full,
}

/// One bounded staging buffer. Its allocation lease must be kept with the
/// resulting ObjectBatchInput until installation returns.
pub(super) struct LiteralBlockBuilder {
    bytes: Vec<u8>,
    restarts: Vec<u16>,
    count: usize,
    first: Option<[u8; KEY_BYTES]>,
    last: Option<[u8; KEY_BYTES]>,
    memory: Option<MemoryLease>,
}

impl LiteralBlockBuilder {
    pub(super) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(
                MAX_BLOCK_BYTES
                    .checked_add(
                        MAX_RESTARTS
                            .checked_mul(std::mem::size_of::<u16>())
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(MAX_BLOCK_BYTES)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        bytes.extend_from_slice(&MAGIC);
        bytes.push(VERSION_V2);
        bytes.extend_from_slice(&[0; 8]);
        let mut restarts = Vec::new();
        restarts
            .try_reserve_exact(MAX_RESTARTS)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        Ok(Self {
            bytes,
            restarts,
            count: 0,
            first: None,
            last: None,
            memory,
        })
    }

    pub(super) fn try_push(
        &mut self,
        key: [u8; KEY_BYTES],
        budget: &mut WorkBudget,
    ) -> Result<PushResult, IndexError> {
        budget.checkpoint()?;
        if self.last.is_some_and(|prior| prior >= key) {
            return Err(IndexError::InvalidInput(
                "literal block keys are not strictly sorted".into(),
            ));
        }
        let restart = self.count.is_multiple_of(RESTART_INTERVAL);
        let prefix = if restart {
            0
        } else {
            common_prefix(
                &self.last.ok_or_else(|| {
                    IndexError::BackendViolation("literal block lost prior key".into())
                })?,
                &key,
            )
        };
        let row_bytes = KEY_BYTES
            .checked_sub(prefix)
            .and_then(|suffix| suffix.checked_add(1))
            .ok_or(IndexError::ResourceLimited)?;
        let next_restart_bytes = self
            .restarts
            .len()
            .checked_add(usize::from(restart))
            .and_then(|count| count.checked_mul(RESTART_BYTES))
            .ok_or(IndexError::ResourceLimited)?;
        let next_total = self
            .bytes
            .len()
            .checked_add(row_bytes)
            .and_then(|width| width.checked_add(next_restart_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        if self.count == usize::from(u16::MAX) || next_total > MAX_BLOCK_BYTES {
            return Ok(PushResult::Full);
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                row_bytes
                    .checked_add(if restart { RESTART_BYTES } else { 0 })
                    .and_then(|width| {
                        width.checked_add(if self.count == 0 { V2_HEADER_BYTES } else { 0 })
                    })
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_width| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if restart {
            let offset = self
                .bytes
                .len()
                .checked_sub(V2_HEADER_BYTES)
                .ok_or(IndexError::ResourceLimited)?;
            self.restarts
                .push(u16::try_from(offset).map_err(|_width| IndexError::ResourceLimited)?);
        }
        self.bytes
            .push(u8::try_from(prefix).map_err(|_width| IndexError::ResourceLimited)?);
        self.bytes.extend_from_slice(
            key.get(prefix..).ok_or_else(|| {
                IndexError::BackendViolation("literal block suffix absent".into())
            })?,
        );
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let _first = self.first.get_or_insert(key);
        self.last = Some(key);
        Ok(PushResult::Added)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub(super) fn finish(mut self) -> Result<PreparedLiteralBlock, IndexError> {
        let first = self
            .first
            .ok_or_else(|| IndexError::InvalidInput("literal block cannot be empty".into()))?;
        let last = self
            .last
            .ok_or_else(|| IndexError::BackendViolation("literal block lost last key".into()))?;
        let count = u16::try_from(self.count).map_err(|_width| IndexError::ResourceLimited)?;
        let restart_count =
            u16::try_from(self.restarts.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let body_bytes = self
            .bytes
            .len()
            .checked_sub(V2_HEADER_BYTES)
            .ok_or(IndexError::ResourceLimited)?;
        self.bytes
            .get_mut(5..7)
            .ok_or_else(|| IndexError::BackendViolation("literal block header absent".into()))?
            .copy_from_slice(&count.to_be_bytes());
        self.bytes
            .get_mut(7..9)
            .ok_or_else(|| IndexError::BackendViolation("literal restart header absent".into()))?
            .copy_from_slice(&restart_count.to_be_bytes());
        self.bytes
            .get_mut(9..13)
            .ok_or_else(|| IndexError::BackendViolation("literal body header absent".into()))?
            .copy_from_slice(
                &u32::try_from(body_bytes)
                    .map_err(|_width| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
        for offset in self.restarts {
            self.bytes.extend_from_slice(&offset.to_be_bytes());
        }
        if self.bytes.len() > MAX_BLOCK_BYTES {
            return Err(IndexError::BackendViolation(
                "literal block exceeds byte cap".into(),
            ));
        }
        Ok(PreparedLiteralBlock {
            bytes: self.bytes,
            first_key: first,
            last_key: last,
            count,
            memory: self.memory,
        })
    }
}

pub(super) struct PreparedLiteralBlock {
    bytes: Vec<u8>,
    pub(super) first_key: [u8; KEY_BYTES],
    pub(super) last_key: [u8; KEY_BYTES],
    pub(super) count: u16,
    memory: Option<MemoryLease>,
}

impl PreparedLiteralBlock {
    /// Keep the returned grant alive through `put_batch`/`install_batch`.
    pub(super) fn into_batch_input(self) -> (ObjectBatchInput, Option<MemoryLease>) {
        (
            ObjectBatchInput {
                payload: self.bytes,
                children: Vec::new(),
            },
            self.memory,
        )
    }
}

/// StoredObject retains the object-port read allocation grant.
pub(super) struct LiteralBlock {
    stored: StoredObject,
    count: usize,
    wire: BlockWire,
}

struct Restart {
    offset: usize,
    key: [u8; KEY_BYTES],
}

enum BlockWire {
    Flat,
    Front {
        body_end: usize,
        restarts: Vec<Restart>,
        _memory: Option<MemoryLease>,
    },
}

#[derive(Clone, Copy, Default)]
struct KeyState {
    index: usize,
    offset: usize,
    previous: Option<[u8; KEY_BYTES]>,
}

fn common_prefix(left: &[u8; KEY_BYTES], right: &[u8; KEY_BYTES]) -> usize {
    left.iter().zip(right).take_while(|(a, b)| a == b).count()
}

impl LiteralBlock {
    fn decode(stored: StoredObject, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let bytes = stored.payload.as_slice();
        if bytes.len() > MAX_BLOCK_BYTES || bytes.get(..4) != Some(MAGIC.as_slice()) {
            return Err(corrupt("literal block magic or byte admission differs"));
        }
        let count_bytes: [u8; 2] = bytes
            .get(5..7)
            .ok_or_else(|| corrupt("literal block count absent"))?
            .try_into()
            .map_err(|_width| corrupt("literal block count width differs"))?;
        let count = usize::from(u16::from_be_bytes(count_bytes));
        if count == 0 {
            return Err(corrupt("literal block is empty"));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes.len()).map_err(|_width| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let wire = match bytes.get(4) {
            Some(&VERSION_V1) => {
                let expected = V1_HEADER_BYTES
                    .checked_add(
                        count
                            .checked_mul(KEY_BYTES)
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                if count > MAX_V1_KEYS || expected != bytes.len() {
                    return Err(corrupt("flat literal block width differs"));
                }
                let mut prior = None;
                for index in 0..count {
                    budget.checkpoint()?;
                    let key = flat_key_at(bytes, index)
                        .ok_or_else(|| corrupt("flat literal key absent"))?;
                    if prior.is_some_and(|old| old >= key) {
                        return Err(corrupt("flat literal keys are not strictly sorted"));
                    }
                    prior = Some(key);
                }
                BlockWire::Flat
            }
            Some(&VERSION_V2) => {
                let restart_bytes: [u8; 2] = bytes
                    .get(7..9)
                    .ok_or_else(|| corrupt("literal restart count absent"))?
                    .try_into()
                    .map_err(|_width| corrupt("literal restart count width differs"))?;
                let body_bytes: [u8; 4] = bytes
                    .get(9..13)
                    .ok_or_else(|| corrupt("literal body width absent"))?
                    .try_into()
                    .map_err(|_width| corrupt("literal body width differs"))?;
                let restart_count = usize::from(u16::from_be_bytes(restart_bytes));
                let body_len = usize::try_from(u32::from_be_bytes(body_bytes))
                    .map_err(|_width| IndexError::ResourceLimited)?;
                let expected_restarts = count.div_ceil(RESTART_INTERVAL);
                let table_bytes = restart_count
                    .checked_mul(RESTART_BYTES)
                    .ok_or(IndexError::ResourceLimited)?;
                let body_end = V2_HEADER_BYTES
                    .checked_add(body_len)
                    .ok_or(IndexError::ResourceLimited)?;
                let total = body_end
                    .checked_add(table_bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                if restart_count != expected_restarts
                    || restart_count == 0
                    || restart_count > MAX_RESTARTS
                    || total != bytes.len()
                {
                    return Err(corrupt("literal restart count or sections differ"));
                }
                let memory = budget.reserve_memory(
                    u64::try_from(
                        restart_count
                            .checked_mul(std::mem::size_of::<Restart>())
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_width| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?;
                let mut restarts = Vec::new();
                restarts
                    .try_reserve_exact(restart_count)
                    .map_err(|_allocation| IndexError::ResourceLimited)?;
                for index in 0..restart_count {
                    budget.checkpoint()?;
                    let at = body_end
                        .checked_add(
                            index
                                .checked_mul(RESTART_BYTES)
                                .ok_or(IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)?;
                    let end = at
                        .checked_add(RESTART_BYTES)
                        .ok_or(IndexError::ResourceLimited)?;
                    let offset_bytes: [u8; 2] = bytes
                        .get(at..end)
                        .ok_or_else(|| corrupt("literal restart offset absent"))?
                        .try_into()
                        .map_err(|_width| corrupt("literal restart offset width differs"))?;
                    let offset = usize::from(u16::from_be_bytes(offset_bytes));
                    let start = V2_HEADER_BYTES
                        .checked_add(offset)
                        .ok_or(IndexError::ResourceLimited)?;
                    let key_start = start.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    let key_end = key_start
                        .checked_add(KEY_BYTES)
                        .ok_or(IndexError::ResourceLimited)?;
                    if offset >= body_len
                        || (index == 0 && offset != 0)
                        || restarts
                            .last()
                            .is_some_and(|prior: &Restart| prior.offset >= offset)
                        || bytes.get(start) != Some(&0)
                    {
                        return Err(corrupt("literal restart offset or prefix differs"));
                    }
                    let key: [u8; KEY_BYTES] = bytes
                        .get(key_start..key_end)
                        .filter(|_| key_end <= body_end)
                        .ok_or_else(|| corrupt("literal restart key exceeds body"))?
                        .try_into()
                        .map_err(|_width| corrupt("literal restart key width differs"))?;
                    if restarts
                        .last()
                        .is_some_and(|prior: &Restart| prior.key >= key)
                    {
                        return Err(corrupt("literal restart keys are not strictly sorted"));
                    }
                    restarts.push(Restart { offset, key });
                }
                BlockWire::Front {
                    body_end,
                    restarts,
                    _memory: memory,
                }
            }
            _ => return Err(corrupt("literal block version differs")),
        };
        Ok(Self {
            stored,
            count,
            wire,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.count
    }

    fn state_at_restart(&self, group: usize) -> Result<KeyState, IndexError> {
        let BlockWire::Front { restarts, .. } = &self.wire else {
            return Err(corrupt("flat block has no restart"));
        };
        let offset = restarts
            .get(group)
            .ok_or_else(|| corrupt("literal restart group absent"))?
            .offset;
        let index = group
            .checked_mul(RESTART_INTERVAL)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(KeyState {
            index,
            offset,
            previous: None,
        })
    }

    fn next_key(&self, state: &mut KeyState) -> Result<Option<[u8; KEY_BYTES]>, IndexError> {
        if state.index == self.count {
            if let BlockWire::Front { body_end, .. } = &self.wire
                && V2_HEADER_BYTES.checked_add(state.offset) != Some(*body_end)
            {
                return Err(corrupt("literal front-coded body has trailing bytes"));
            }
            return Ok(None);
        }
        let key = match &self.wire {
            BlockWire::Flat => flat_key_at(&self.stored.payload, state.index)
                .ok_or_else(|| corrupt("flat literal key absent"))?,
            BlockWire::Front {
                body_end, restarts, ..
            } => {
                let restart = state.index.is_multiple_of(RESTART_INTERVAL);
                if restart {
                    let group = state.index / RESTART_INTERVAL;
                    let expected = restarts
                        .get(group)
                        .ok_or_else(|| corrupt("literal restart index absent"))?;
                    if expected.offset != state.offset {
                        return Err(corrupt("literal restart row offset differs"));
                    }
                }
                let start = V2_HEADER_BYTES
                    .checked_add(state.offset)
                    .ok_or(IndexError::ResourceLimited)?;
                let prefix = usize::from(
                    *self
                        .stored
                        .payload
                        .get(start)
                        .ok_or_else(|| corrupt("literal front-coded prefix absent"))?,
                );
                if (restart && prefix != 0)
                    || (!restart && (prefix >= KEY_BYTES || state.previous.is_none()))
                {
                    return Err(corrupt("literal front-coded prefix differs"));
                }
                let suffix_len = KEY_BYTES
                    .checked_sub(prefix)
                    .ok_or(IndexError::ResourceLimited)?;
                let suffix_start = start.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let suffix_end = suffix_start
                    .checked_add(suffix_len)
                    .ok_or(IndexError::ResourceLimited)?;
                let suffix = self
                    .stored
                    .payload
                    .get(suffix_start..suffix_end)
                    .filter(|_| suffix_end <= *body_end)
                    .ok_or_else(|| corrupt("literal front-coded suffix exceeds body"))?;
                let mut key = [0; KEY_BYTES];
                if prefix != 0 {
                    let previous = state
                        .previous
                        .ok_or_else(|| corrupt("literal front-coded prior key absent"))?;
                    key.get_mut(..prefix)
                        .ok_or_else(|| corrupt("literal prefix destination absent"))?
                        .copy_from_slice(
                            previous
                                .get(..prefix)
                                .ok_or_else(|| corrupt("literal prior prefix absent"))?,
                        );
                }
                key.get_mut(prefix..)
                    .ok_or_else(|| corrupt("literal suffix destination absent"))?
                    .copy_from_slice(suffix);
                if !restart
                    && state
                        .previous
                        .is_some_and(|previous| common_prefix(&previous, &key) != prefix)
                {
                    return Err(corrupt("literal front-coded prefix is not maximal"));
                }
                state.offset = suffix_end
                    .checked_sub(V2_HEADER_BYTES)
                    .ok_or(IndexError::ResourceLimited)?;
                key
            }
        };
        if state.previous.is_some_and(|prior| prior >= key) {
            return Err(corrupt("literal block keys are not strictly sorted"));
        }
        state.index = state
            .index
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        state.previous = Some(key);
        if let BlockWire::Front {
            body_end, restarts, ..
        } = &self.wire
        {
            if state.index == self.count
                && V2_HEADER_BYTES.checked_add(state.offset) != Some(*body_end)
            {
                return Err(corrupt("literal front-coded body extent differs"));
            }
            if state.index.is_multiple_of(RESTART_INTERVAL) && state.index < self.count {
                let group = state.index / RESTART_INTERVAL;
                let next = restarts
                    .get(group)
                    .ok_or_else(|| corrupt("next literal restart absent"))?;
                if state.offset != next.offset || key >= next.key {
                    return Err(corrupt("literal restart boundary or order differs"));
                }
            }
        }
        Ok(Some(key))
    }

    pub(super) fn key_at(&self, index: usize) -> Result<Option<[u8; KEY_BYTES]>, IndexError> {
        if index >= self.len() {
            return Ok(None);
        }
        if matches!(&self.wire, BlockWire::Flat) {
            return flat_key_at(&self.stored.payload, index)
                .map(Some)
                .ok_or_else(|| corrupt("flat literal key absent"));
        }
        let mut state = self.state_at_restart(index / RESTART_INTERVAL)?;
        while state.index <= index {
            let key = self
                .next_key(&mut state)?
                .ok_or_else(|| corrupt("literal selected key absent"))?;
            if state.index == index.checked_add(1).ok_or(IndexError::ResourceLimited)? {
                return Ok(Some(key));
            }
        }
        Err(corrupt("literal selected key not reached"))
    }

    pub(super) fn first_key(&self) -> Result<[u8; KEY_BYTES], IndexError> {
        self.key_at(0)?
            .ok_or_else(|| corrupt("literal block first key absent"))
    }

    pub(super) fn last_key(&self) -> Result<[u8; KEY_BYTES], IndexError> {
        let index = self
            .count
            .checked_sub(1)
            .ok_or_else(|| corrupt("literal block last index absent"))?;
        self.key_at(index)?
            .ok_or_else(|| corrupt("literal block last key absent"))
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = Result<[u8; KEY_BYTES], IndexError>> + '_ {
        LiteralKeyIter {
            block: self,
            state: KeyState::default(),
            failed: false,
            finished: false,
        }
    }

    #[cfg(test)]
    fn validate_all(&self) -> Result<(), IndexError> {
        for key in self.keys() {
            let _validated = key?;
        }
        Ok(())
    }

    fn seek(
        &self,
        target: &[u8; KEY_BYTES],
    ) -> Result<(usize, Option<[u8; KEY_BYTES]>), IndexError> {
        match &self.wire {
            BlockWire::Flat => {
                let mut lower = 0_usize;
                let mut upper = self.count;
                while lower < upper {
                    let half = upper
                        .checked_sub(lower)
                        .ok_or(IndexError::ResourceLimited)?
                        >> 1;
                    let middle = lower.checked_add(half).ok_or(IndexError::ResourceLimited)?;
                    let key = flat_key_at(&self.stored.payload, middle)
                        .ok_or_else(|| corrupt("flat literal search key absent"))?;
                    if key < *target {
                        lower = middle.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    } else {
                        upper = middle;
                    }
                }
                Ok((lower, self.key_at(lower)?))
            }
            BlockWire::Front { restarts, .. } => {
                let group = restarts
                    .partition_point(|restart| restart.key <= *target)
                    .saturating_sub(1);
                let mut state = self.state_at_restart(group)?;
                for _ in 0..RESTART_INTERVAL {
                    let index = state.index;
                    let Some(key) = self.next_key(&mut state)? else {
                        return Ok((index, None));
                    };
                    if key >= *target {
                        return Ok((index, Some(key)));
                    }
                    if state.index.is_multiple_of(RESTART_INTERVAL) {
                        break;
                    }
                }
                let next = restarts
                    .get(group.checked_add(1).ok_or(IndexError::ResourceLimited)?)
                    .map(|restart| restart.key);
                Ok((state.index, next))
            }
        }
    }

    pub(super) fn contains(&self, target: &[u8; KEY_BYTES]) -> Result<bool, IndexError> {
        Ok(self.seek(target)?.1 == Some(*target))
    }

    fn lower_bound(&self, target: &[u8; KEY_BYTES]) -> Result<usize, IndexError> {
        self.seek(target).map(|(index, _key)| index)
    }

    fn state_at_index(&self, index: usize) -> Result<KeyState, IndexError> {
        if index > self.count {
            return Err(corrupt("literal cursor index exceeds block"));
        }
        if matches!(&self.wire, BlockWire::Flat) {
            return Ok(KeyState {
                index,
                ..KeyState::default()
            });
        }
        if index == self.count {
            let BlockWire::Front { body_end, .. } = &self.wire else {
                return Err(corrupt("literal front cursor wire differs"));
            };
            return Ok(KeyState {
                index,
                offset: body_end
                    .checked_sub(V2_HEADER_BYTES)
                    .ok_or(IndexError::ResourceLimited)?,
                previous: None,
            });
        }
        let mut state = self.state_at_restart(index / RESTART_INTERVAL)?;
        while state.index < index {
            let _prior = self
                .next_key(&mut state)?
                .ok_or_else(|| corrupt("literal cursor seek ended early"))?;
        }
        Ok(state)
    }
}

struct LiteralKeyIter<'a> {
    block: &'a LiteralBlock,
    state: KeyState,
    failed: bool,
    finished: bool,
}

impl Iterator for LiteralKeyIter<'_> {
    type Item = Result<[u8; KEY_BYTES], IndexError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.finished {
            return None;
        }
        match self.block.next_key(&mut self.state) {
            Ok(Some(key)) => Some(Ok(key)),
            Ok(None) => {
                self.finished = true;
                None
            }
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

fn flat_key_at(bytes: &[u8], index: usize) -> Option<[u8; KEY_BYTES]> {
    let start = V1_HEADER_BYTES.checked_add(index.checked_mul(KEY_BYTES)?)?;
    let end = start.checked_add(KEY_BYTES)?;
    bytes.get(start..end)?.try_into().ok()
}

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(reason.into())
}

fn predecessor(mut key: [u8; KEY_BYTES]) -> Option<[u8; KEY_BYTES]> {
    for position in (0..KEY_BYTES).rev() {
        let byte = key.get_mut(position)?;
        if *byte == 0 {
            continue;
        }
        *byte = byte.checked_sub(1)?;
        for suffix in key.get_mut(position.checked_add(1)?..)? {
            *suffix = u8::MAX;
        }
        return Some(key);
    }
    None
}

fn lower_bound_page(
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    target: &[u8; KEY_BYTES],
    budget: &mut WorkBudget,
) -> Result<Option<([u8; KEY_BYTES], ObjectRef)>, IndexError> {
    let after = predecessor(*target);
    let page = maps.scan(
        root,
        after.as_ref().map(<[u8; KEY_BYTES]>::as_slice),
        1,
        budget,
    )?;
    let Some((key, value)) = page.entries.first() else {
        if page.has_more {
            return Err(IndexError::BackendViolation(
                "empty literal directory scan has continuation".into(),
            ));
        }
        return Ok(None);
    };
    if page.entries.len() != 1 {
        return Err(IndexError::BackendViolation(
            "literal directory scan exceeds one entry".into(),
        ));
    }
    let last: [u8; KEY_BYTES] = key
        .as_slice()
        .try_into()
        .map_err(|_width| corrupt("literal directory key width differs"))?;
    if last < *target {
        return Err(corrupt("literal directory lower bound regressed"));
    }
    let reference = match value {
        MapValue::Object(reference) => *reference,
        MapValue::Inline(_) => return Err(corrupt("literal directory value is inline")),
    };
    Ok(Some((last, reference)))
}

/// Authenticated read of one exact last-key directory entry. The caller may
/// use `keys()` to compare all members with an independent transition oracle.
pub(super) fn load_block(
    objects: &dyn ImmutableObjectPort,
    last: [u8; KEY_BYTES],
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<LiteralBlock, IndexError> {
    budget.checkpoint()?;
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference
        || !stored.registration.children.as_slice().is_empty()
    {
        return Err(corrupt("literal block object identity or children differ"));
    }
    let block = LiteralBlock::decode(stored, budget)?;
    if block.last_key()? != last {
        return Err(corrupt("literal directory key differs from block last key"));
    }
    Ok(block)
}

pub(super) fn contains(
    maps: &dyn PersistentMapPort,
    objects: &dyn ImmutableObjectPort,
    root: MapRoot,
    target: [u8; KEY_BYTES],
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let Some((last, reference)) = lower_bound_page(maps, root, &target, budget)? else {
        return Ok(false);
    };
    let block = load_block(objects, last, reference, budget)?;
    block.contains(&target)
}

pub(super) struct LiteralGramCursor<'a> {
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
    root: MapRoot,
    gram: [u8; 3],
    lower: [u8; KEY_BYTES],
    exclusive_lower: bool,
    after_block: Option<[u8; KEY_BYTES]>,
    block: Option<LiteralBlock>,
    state: KeyState,
    prior_last: Option<[u8; KEY_BYTES]>,
    ended: bool,
}

impl<'a> LiteralGramCursor<'a> {
    pub(super) fn new(
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
        root: MapRoot,
        gram: [u8; 3],
        after_owner: Option<[u8; 32]>,
    ) -> Self {
        let lower = literal_key(gram, after_owner.unwrap_or([0; 32]));
        Self {
            maps,
            objects,
            root,
            gram,
            lower,
            exclusive_lower: after_owner.is_some(),
            after_block: None,
            block: None,
            state: KeyState::default(),
            prior_last: None,
            ended: false,
        }
    }

    pub(super) fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<[u8; 32]>, IndexError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            budget.checkpoint()?;
            if let Some(block) = &self.block {
                let mut next_state = self.state;
                if let Some(key) = block.next_key(&mut next_state)? {
                    let gram: [u8; 3] = key
                        .get(..3)
                        .ok_or_else(|| corrupt("literal block gram absent"))?
                        .try_into()
                        .map_err(|_width| corrupt("literal block gram width differs"))?;
                    if gram > self.gram {
                        self.ended = true;
                        return Ok(None);
                    }
                    if gram == self.gram
                        && (key > self.lower || (!self.exclusive_lower && key == self.lower))
                    {
                        let owner: [u8; 32] = key
                            .get(3..)
                            .ok_or_else(|| corrupt("literal block owner absent"))?
                            .try_into()
                            .map_err(|_width| corrupt("literal block owner width differs"))?;
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(KEY_BYTES)
                                .map_err(|_width| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        self.state = next_state;
                        return Ok(Some(owner));
                    }
                    self.state = next_state;
                    continue;
                }
                self.block = None;
            }
            let (last, reference) = if let Some(after) = self.after_block {
                let page = self.maps.scan(self.root, Some(&after), 1, budget)?;
                let Some((key, value)) = page.entries.first() else {
                    if page.has_more {
                        return Err(IndexError::BackendViolation(
                            "empty literal directory continuation".into(),
                        ));
                    }
                    self.ended = true;
                    return Ok(None);
                };
                if page.entries.len() != 1 {
                    return Err(IndexError::BackendViolation(
                        "literal directory continuation exceeds one entry".into(),
                    ));
                }
                let key: [u8; KEY_BYTES] = key
                    .as_slice()
                    .try_into()
                    .map_err(|_width| corrupt("literal directory key width differs"))?;
                if key <= after {
                    return Err(corrupt("literal directory continuation regressed"));
                }
                let reference = match value {
                    MapValue::Object(reference) => *reference,
                    MapValue::Inline(_) => {
                        return Err(corrupt("literal directory value is inline"));
                    }
                };
                (key, reference)
            } else {
                let Some(result) = lower_bound_page(self.maps, self.root, &self.lower, budget)?
                else {
                    self.ended = true;
                    return Ok(None);
                };
                result
            };
            let block = load_block(self.objects, last, reference, budget)?;
            let first = block.first_key()?;
            if self.prior_last.is_some_and(|prior| prior >= first) {
                return Err(corrupt("literal directory block ranges overlap"));
            }
            self.after_block = Some(last);
            self.prior_last = Some(last);
            if first
                .get(..3)
                .is_some_and(|prefix| prefix > self.gram.as_slice())
            {
                self.ended = true;
                return Ok(None);
            }
            let index = block.lower_bound(&self.lower)?;
            self.state = block.state_at_index(index)?;
            self.block = Some(block);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use quanta_index_core::index::{Digest, RegisteredObject, WorkLimits};

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .expect("deadline"),
        )
    }

    fn stored(payload: Vec<u8>) -> StoredObject {
        let reference = ObjectRef(Digest([3; 32]));
        StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                u64::try_from(payload.len()).expect("width"),
                Vec::new(),
            ),
            payload,
            None,
        )
    }

    #[test]
    fn canonical_fixed_width_block_and_membership() {
        let mut budget = budget();
        let first = literal_key(*b"abc", [0; 32]);
        let mut last_owner = [0; 32];
        *last_owner.last_mut().expect("last byte") = 1;
        let last = literal_key(*b"abc", last_owner);
        let mut builder = LiteralBlockBuilder::new(&budget).expect("builder");
        assert!(builder.is_empty());
        assert_eq!(
            builder.try_push(first, &mut budget).expect("first"),
            PushResult::Added
        );
        assert!(!builder.is_empty());
        assert_eq!(
            builder.try_push(last, &mut budget).expect("last"),
            PushResult::Added
        );
        assert!(builder.try_push(last, &mut budget).is_err());
        let prepared = builder.finish().expect("prepared");
        assert_eq!(
            (prepared.first_key, prepared.last_key, prepared.count),
            (first, last, 2)
        );
        let (input, _memory) = prepared.into_batch_input();
        assert!(input.children.is_empty());
        let block = LiteralBlock::decode(stored(input.payload), &mut budget).expect("block");
        assert_eq!(block.len(), 2);
        assert_eq!(block.first_key().expect("first"), first);
        assert_eq!(block.last_key().expect("last"), last);
        assert_eq!(
            block.keys().collect::<Result<Vec<_>, _>>().expect("keys"),
            vec![first, last]
        );
        assert!(block.contains(&first).expect("first membership"));
        assert!(block.contains(&last).expect("last membership"));
        assert!(
            !block
                .contains(&literal_key(*b"abd", [0; 32]))
                .expect("absence")
        );
        block.validate_all().expect("complete v2 validation");
    }

    #[test]
    fn flat_v1_remains_readable_and_v2_restart_corruption_refuses() {
        let mut budget = budget();
        let first = literal_key(*b"abc", [0; 32]);
        let mut owner = [0; 32];
        *owner.last_mut().expect("owner") = 1;
        let last = literal_key(*b"abc", owner);
        let mut v1 = Vec::new();
        v1.extend_from_slice(&MAGIC);
        v1.push(VERSION_V1);
        v1.extend_from_slice(&2_u16.to_be_bytes());
        v1.extend_from_slice(&first);
        v1.extend_from_slice(&last);
        let old = LiteralBlock::decode(stored(v1), &mut budget).expect("flat v1");
        assert_eq!(
            old.keys().collect::<Result<Vec<_>, _>>().expect("keys"),
            vec![first, last]
        );
        assert!(old.contains(&last).expect("old point"));

        let mut builder = LiteralBlockBuilder::new(&budget).expect("builder");
        assert_eq!(
            builder.try_push(first, &mut budget).expect("push"),
            PushResult::Added
        );
        assert_eq!(
            builder.try_push(last, &mut budget).expect("push"),
            PushResult::Added
        );
        let (input, _memory) = builder.finish().expect("block").into_batch_input();
        let mut bad_offset = input.payload.clone();
        let offset_at = bad_offset.len().checked_sub(2).expect("offset");
        bad_offset
            .get_mut(offset_at..)
            .expect("offset bytes")
            .copy_from_slice(&1_u16.to_be_bytes());
        assert!(LiteralBlock::decode(stored(bad_offset), &mut budget).is_err());
        let mut bad_prefix = input.payload;
        let second_prefix = V2_HEADER_BYTES
            .checked_add(KEY_BYTES)
            .and_then(|offset| offset.checked_add(1))
            .expect("second prefix");
        *bad_prefix.get_mut(second_prefix).expect("prefix") = 0;
        let block = LiteralBlock::decode(stored(bad_prefix), &mut budget)
            .expect("structural header remains valid");
        assert!(block.validate_all().is_err());
    }

    #[test]
    fn full_block_is_bounded_and_preserves_next_key() {
        let mut budget = budget();
        let mut builder = LiteralBlockBuilder::new(&budget).expect("builder");
        let mut accepted = 0_u16;
        loop {
            let mut owner = [0; 32];
            let suffix = accepted.to_be_bytes();
            owner
                .get_mut(30..)
                .expect("owner suffix")
                .copy_from_slice(&suffix);
            match builder
                .try_push(literal_key(*b"abc", owner), &mut budget)
                .expect("push")
            {
                PushResult::Added => accepted = accepted.checked_add(1).expect("test count"),
                PushResult::Full => break,
            }
        }
        let prepared = builder.finish().expect("block");
        assert_eq!(prepared.count, accepted);
        assert!(usize::from(accepted) > MAX_V1_KEYS);
        let (input, _memory) = prepared.into_batch_input();
        assert!(input.payload.len() <= MAX_BLOCK_BYTES);
    }

    #[test]
    fn fixed_width_predecessor_is_immediate() {
        assert_eq!(predecessor([0; 35]), None);
        let mut key = [0; 35];
        *key.get_mut(3).expect("byte") = 1;
        let prior = predecessor(key).expect("predecessor");
        assert_eq!(prior.get(3), Some(&0));
        assert!(
            prior
                .get(4..)
                .expect("suffix")
                .iter()
                .all(|byte| *byte == 255)
        );
    }
}
