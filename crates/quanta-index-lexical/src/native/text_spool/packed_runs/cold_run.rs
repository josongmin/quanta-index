//! A bounded term-major run over the checked owner-major cold stream.

#[expect(
    clippy::disallowed_types,
    reason = "existing packet-local hash chains define charged comparisons"
)]
use std::hash::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::mem::size_of;

use super::corrupt;
use super::{
    EDIT_HEADER, GROUPED_EDIT_MAGIC, MAX_EDIT_ROWS, MAX_TERM_KEY_BYTES, MapValue, MemoryLifetime,
    PACKET_BYTES, PACKET_WRITER_MEMORY, PacketWriter, PostingPacketFences, REPEATED_TERM,
    RowLayout, RowOffset, RowView, TextSpool, WorkBudget, WorkUsage, finalize_compact_packet,
    measured_copy, observe_packet_work, width,
};
use quanta_index_core::index::IndexError;
use std::cmp::Ordering;
#[expect(
    clippy::disallowed_types,
    reason = "bounded cold-run lookup; chain order defines output"
)]
use std::collections::HashMap;

const END_OF_CHAIN: u16 = u16::MAX;
// Each repeated cold row stores a 2-byte term marker, a 2-byte owner,
// an absent old value and a 3-byte object reference.
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "the fixed 1024-byte key and u16 marker fit usize on supported targets"
)]
const _: () = assert!(8 * MAX_EDIT_ROWS + (MAX_TERM_KEY_BYTES as usize) < REPEATED_TERM as usize);

#[derive(Clone, Copy)]
struct TermKey {
    start: u32,
    length: u16,
}

struct TermChain {
    key: TermKey,
    first: u16,
    last: u16,
    same_hash: Option<u16>,
}

struct ColdRow {
    owner: u16,
    reference: u16,
    next: u16,
}

// Reserve the same Original envelope as the general pending writer. The
// bound covers the output-size prefix and the interned-key arena, every row
// and offset, up to one owner and two references, and four hash indices.
const _: () = assert!(
    2 * PACKET_BYTES
        + MAX_EDIT_ROWS
            * (size_of::<RowOffset>()
                + size_of::<ColdRow>()
                + size_of::<TermChain>()
                + size_of::<u16>()
                + 3 * 32
                + 4 * 64)
        <= PACKET_WRITER_MEMORY
);

pub(super) struct ColdRun {
    keys: Vec<u8>,
    terms: Vec<TermChain>,
    rows: Vec<ColdRow>,
    #[expect(clippy::disallowed_types, reason = "bounded index; no map iteration")]
    lookup: HashMap<u64, u16>,
    order: Vec<u16>,
    // Repeated term bytes count toward the output cap, even if interned.
    encoded_end: usize,
    repeated_term_bytes: usize,
    // Holds the shared grant and drops after the staging arrays.
    pub(super) packet: PacketWriter,
}

impl ColdRun {
    pub(super) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let packet = PacketWriter::new(budget, MemoryLifetime::Retained)?;
        let mut keys = Vec::new();
        keys.try_reserve_exact(PACKET_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut terms = Vec::new();
        terms
            .try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        #[expect(
            clippy::disallowed_types,
            reason = "preallocated within packet Original grant"
        )]
        let mut lookup = HashMap::new();
        lookup
            .try_reserve(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut order = Vec::new();
        order
            .try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            keys,
            terms,
            rows,
            lookup,
            order,
            encoded_end: EDIT_HEADER,
            repeated_term_bytes: 0,
            packet,
        })
    }

    pub(super) fn plan_row(&self, row: &RowView<'_>) -> Result<Option<RowLayout>, IndexError> {
        self.packet.plan_row_at(row, self.encoded_end)
    }

    fn key(&self, key: TermKey) -> Result<&[u8], IndexError> {
        let start = usize::try_from(key.start).map_err(|_error| IndexError::ResourceLimited)?;
        let end = start
            .checked_add(usize::from(key.length))
            .ok_or(IndexError::ResourceLimited)?;
        self.keys.get(start..end).ok_or(IndexError::ResourceLimited)
    }

    fn find_term(
        &self,
        term: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(u64, Option<u16>), IndexError> {
        budget.charge(WorkUsage {
            bytes: width(term.len())?,
            ..WorkUsage::default()
        })?;
        // Keep the charged collision chain reproducible across requests.
        // HashMap may still randomize its internal placement of the index.
        #[expect(
            clippy::disallowed_types,
            reason = "preserve existing charged collision chains"
        )]
        let mut hasher = DefaultHasher::new();
        term.hash(&mut hasher);
        let hash = hasher.finish();
        let mut id = self.lookup.get(&hash).copied();
        // Hashes identify probe chains only. Every hit checks the full key.
        while let Some(current) = id {
            let chain = self
                .terms
                .get(usize::from(current))
                .ok_or(IndexError::ResourceLimited)?;
            let key = self.key(chain.key)?;
            budget.charge(WorkUsage {
                bytes: width(key.len().min(term.len()))?,
                ..WorkUsage::default()
            })?;
            if key == term {
                return Ok((hash, id));
            }
            id = chain.same_hash;
        }
        Ok((hash, None))
    }

    fn stage_charge(term: Option<u16>, copy_key: usize) -> Result<usize, IndexError> {
        let links = if term.is_some() {
            size_of::<u16>()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
        } else {
            copy_key
                .checked_add(size_of::<TermChain>())
                .and_then(|bytes| bytes.checked_add(size_of::<(u64, u16)>()))
                .ok_or(IndexError::ResourceLimited)?
        };
        size_of::<ColdRow>()
            .checked_add(links)
            .ok_or(IndexError::ResourceLimited)
    }

    fn link_row(
        &mut self,
        owner: u16,
        reference: u16,
        hash: u64,
        term: Option<u16>,
        key: TermKey,
    ) -> Result<(), IndexError> {
        // Both arrays are bounded by MAX_EDIT_ROWS, below the sentinel.
        let row = u16::try_from(self.rows.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let next_term =
            u16::try_from(self.terms.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let prior_last = if let Some(id) = term {
            let last = self
                .terms
                .get(usize::from(id))
                .ok_or(IndexError::ResourceLimited)?
                .last;
            if self.rows.get(usize::from(last)).is_none() {
                return Err(IndexError::ResourceLimited);
            }
            Some(last)
        } else {
            None
        };
        self.rows.push(ColdRow {
            owner,
            reference,
            next: END_OF_CHAIN,
        });
        if let Some(id) = term {
            self.rows
                .get_mut(usize::from(prior_last.ok_or(IndexError::ResourceLimited)?))
                .ok_or(IndexError::ResourceLimited)?
                .next = row;
            let chain = self
                .terms
                .get_mut(usize::from(id))
                .ok_or(IndexError::ResourceLimited)?;
            chain.last = row;
        } else {
            let same_hash = self.lookup.insert(hash, next_term);
            self.terms.push(TermChain {
                key,
                first: row,
                last: row,
                same_hash,
            });
        }
        Ok(())
    }

    pub(super) fn push(
        &mut self,
        row: &RowView<'_>,
        layout: RowLayout,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let (None, Some(MapValue::Object(reference))) = (row.expected, row.next) else {
            return Err(corrupt("cold run requires object insertions"));
        };
        if self.rows.is_empty() {
            let order = self.packet.offsets.last().map_or(Ordering::Less, |&last| {
                let (term, owner) = self.packet.fence(last);
                term.cmp(row.term).then(owner.cmp(&row.owner))
            });
            match order {
                Ordering::Less => {
                    self.packet.push_unordered(row, layout, budget)?;
                    self.encoded_end = self.packet.bytes.len();
                    return Ok(());
                }
                Ordering::Equal => return Err(corrupt("duplicate cold posting key")),
                // Keep the existing prefix in wire order. The suffix needs
                // grouping, but the prefix only needs a linear merge.
                Ordering::Greater => {}
            }
        }
        let (hash, term) = self.find_term(row.term, budget)?;
        if let Some(id) = term {
            let chain = self
                .terms
                .get(usize::from(id))
                .ok_or(IndexError::ResourceLimited)?;
            let last = self
                .rows
                .get(usize::from(chain.last))
                .ok_or(IndexError::ResourceLimited)?;
            if self
                .packet
                .owners
                .get(usize::from(last.owner))
                .ok_or(IndexError::ResourceLimited)?
                >= &row.owner
            {
                return Err(corrupt("duplicate or unordered cold posting key"));
            }
        }
        let key = TermKey {
            start: u32::try_from(self.keys.len()).map_err(|_error| IndexError::ResourceLimited)?,
            length: u16::try_from(row.term.len()).map_err(|_error| IndexError::ResourceLimited)?,
        };
        // The dictionary writes mirror the general writer. Pay for only
        // actual term/row staging here; the encoder charges output separately.
        let stage_bytes = Self::stage_charge(term, row.term.len())?;
        let copy_bytes = layout
            .dictionary_bytes
            .checked_mul(2)
            .and_then(|bytes| stage_bytes.checked_add(bytes))
            .ok_or(IndexError::ResourceLimited)?;
        measured_copy(copy_bytes, budget)?;
        let owner = self.packet.owner_id(row.owner)?;
        let reference = self.packet.reference_id(reference)?;
        if term.is_none() {
            self.keys.extend_from_slice(row.term);
        } else {
            self.repeated_term_bytes = self
                .repeated_term_bytes
                .checked_add(row.term.len())
                .ok_or(IndexError::ResourceLimited)?;
        }
        self.link_row(owner, reference, hash, term, key)?;
        self.encoded_end = self
            .encoded_end
            .checked_add(layout.row_len)
            .ok_or(IndexError::ResourceLimited)?;
        self.packet.count = self
            .packet
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    pub(super) fn finish(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.rows.is_empty() {
            self.packet
                .finish_ordered_pending(spool, phase, run, budget)?;
        } else {
            observe_packet_work(budget, 38, |budget| {
                self.finish_disordered(spool, phase, run, budget)
            })?;
            self.packet.clear_rows();
            self.keys.clear();
            self.terms.clear();
            self.rows.clear();
            self.lookup.clear();
            self.order.clear();
        }
        self.encoded_end = EDIT_HEADER;
        self.repeated_term_bytes = 0;
        Ok(())
    }

    #[expect(
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::arithmetic_side_effects,
        reason = "order IDs and key ranges are created in this bounded run before the infallible sort callback"
    )]
    fn finish_disordered(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        observe_packet_work(budget, 39, |budget| {
            let key_bytes = self
                .terms
                .iter()
                .map(|term| usize::from(term.key.length))
                .sum::<usize>();
            let order_bytes = self
                .terms
                .len()
                .checked_mul(size_of::<u16>())
                .and_then(|bytes| key_bytes.checked_add(bytes))
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes: width(order_bytes)?,
                ..WorkUsage::default()
            })?;
            let count =
                u16::try_from(self.terms.len()).map_err(|_error| IndexError::ResourceLimited)?;
            self.order.clear();
            self.order.extend(0..count);
            let key = |id| {
                let key = self.terms[usize::from(id)].key;
                let start = key.start as usize;
                &self.keys[start..start + usize::from(key.length)]
            };
            self.order
                .sort_unstable_by(|&left, &right| key(left).cmp(key(right)));
            Ok(())
        })?;
        let _output_memory =
            budget.reserve_memory(width(PACKET_BYTES)?, MemoryLifetime::Temporary)?;
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(PACKET_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        ordered.resize(EDIT_HEADER, 0);
        // Run zero can be the final seekable stream. Subsequent phase-zero
        // runs are consumed only by the merge and can encode one key per chain.
        let group_terms = phase == 0 && run > 0 && self.repeated_term_bytes > 0;
        let encoded_end = self
            .encoded_end
            .checked_sub(if group_terms {
                self.repeated_term_bytes
            } else {
                0
            })
            .ok_or(IndexError::ResourceLimited)?;
        measured_copy(
            encoded_end
                .checked_sub(EDIT_HEADER)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        let mut first = None;
        let mut last = None;
        let mut prefix = self.packet.offsets.iter().peekable();
        for &id in &self.order {
            budget.checkpoint()?;
            let chain = self
                .terms
                .get(usize::from(id))
                .ok_or(IndexError::ResourceLimited)?;
            let key = self.key(chain.key)?;
            let first_row = self
                .rows
                .get(usize::from(chain.first))
                .ok_or(IndexError::ResourceLimited)?;
            let first_suffix_owner = *self
                .packet
                .owners
                .get(usize::from(first_row.owner))
                .ok_or(IndexError::ResourceLimited)?;
            // The cold input is owner-major and the prefix precedes the suffix.
            // Once the prefix is past this term, its next key cannot precede
            // any of this term's remaining owners.
            while let Some(&&offset) = prefix.peek() {
                let prior = self.packet.fence(offset);
                let term_order = prior.0.cmp(key);
                let compared = usize::from(term_order == Ordering::Equal)
                    .checked_mul(size_of::<[u8; 32]>())
                    .and_then(|bytes| prior.0.len().min(key.len()).checked_add(bytes))
                    .ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes: width(compared)?,
                    ..WorkUsage::default()
                })?;
                match term_order {
                    Ordering::Greater => break,
                    Ordering::Equal => match prior.1.cmp(&first_suffix_owner) {
                        Ordering::Less => {}
                        Ordering::Equal => return Err(corrupt("duplicate cold posting key")),
                        Ordering::Greater => {
                            return Err(corrupt("cold posting owners are unordered"));
                        }
                    },
                    Ordering::Less => {}
                }
                ordered.extend_from_slice(
                    self.packet
                        .bytes
                        .get(offset.start..offset.end)
                        .ok_or(IndexError::ResourceLimited)?,
                );
                let _first = first.get_or_insert(prior);
                let _offset = prefix.next();
            }
            let mut row_id = chain.first;
            let literal_offset = ordered.len();
            while row_id != END_OF_CHAIN {
                let row = self
                    .rows
                    .get(usize::from(row_id))
                    .ok_or(IndexError::ResourceLimited)?;
                let fence = (
                    key,
                    *self
                        .packet
                        .owners
                        .get(usize::from(row.owner))
                        .ok_or(IndexError::ResourceLimited)?,
                );
                if group_terms && row_id != chain.first {
                    let distance = ordered
                        .len()
                        .checked_sub(literal_offset)
                        .ok_or(IndexError::ResourceLimited)?;
                    let distance =
                        u16::try_from(distance).map_err(|_error| IndexError::ResourceLimited)?;
                    if distance >= REPEATED_TERM {
                        return Err(IndexError::ResourceLimited);
                    }
                    ordered.extend_from_slice(&(REPEATED_TERM | distance).to_be_bytes());
                    ordered.extend_from_slice(&row.owner.to_be_bytes());
                } else {
                    let _offset = PacketWriter::put_row_key(&mut ordered, key, row.owner)?;
                }
                ordered.push(0);
                PacketWriter::put_reference_id(&mut ordered, row.reference);
                let _first = first.get_or_insert(fence);
                last = Some(fence);
                row_id = row.next;
            }
        }
        for &offset in prefix {
            let fence = self.packet.fence(offset);
            ordered.extend_from_slice(
                self.packet
                    .bytes
                    .get(offset.start..offset.end)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let _first = first.get_or_insert(fence);
            last = Some(fence);
        }
        if ordered.len() != encoded_end {
            return Err(corrupt("cold run encoded width differs"));
        }
        finalize_compact_packet(
            &mut ordered,
            self.packet.count,
            &self.packet.owners,
            &self.packet.references,
            budget,
        )?;
        if group_terms {
            ordered
                .get_mut(..8)
                .ok_or(IndexError::ResourceLimited)?
                .copy_from_slice(GROUPED_EDIT_MAGIC);
        }
        spool.insert_posting_packet(
            phase,
            run,
            0,
            &PostingPacketFences {
                first: first.ok_or_else(|| corrupt("cold run has no first key"))?,
                last: last.ok_or_else(|| corrupt("cold run has no last key"))?,
            },
            &ordered,
            budget,
        )
    }
}
