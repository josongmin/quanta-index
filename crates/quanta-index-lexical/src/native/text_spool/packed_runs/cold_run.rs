//! A bounded term-major run over the checked owner-major cold stream.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem::size_of;

use super::*;

const END_OF_CHAIN: u16 = u16::MAX;
// Each repeated cold row stores a 2-byte term marker, a 2-byte owner,
// an absent old value and a 3-byte object reference.
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
            .map_err(|_| IndexError::ResourceLimited)?;
        let mut terms = Vec::new();
        terms
            .try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_| IndexError::ResourceLimited)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_| IndexError::ResourceLimited)?;
        let mut lookup = HashMap::new();
        lookup
            .try_reserve(MAX_EDIT_ROWS)
            .map_err(|_| IndexError::ResourceLimited)?;
        let mut order = Vec::new();
        order
            .try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_| IndexError::ResourceLimited)?;
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

    pub(super) fn plan_row(&self, row: RowView<'_>) -> Result<Option<RowLayout>, IndexError> {
        self.packet.plan_row_at(row, self.encoded_end)
    }

    fn key(&self, key: TermKey) -> &[u8] {
        let start = key.start as usize;
        &self.keys[start..start + usize::from(key.length)]
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
        let mut hasher = DefaultHasher::new();
        term.hash(&mut hasher);
        let hash = hasher.finish();
        let mut id = self.lookup.get(&hash).copied();
        // Hashes identify probe chains only. Every hit checks the full key.
        while let Some(current) = id {
            let chain = &self.terms[usize::from(current)];
            let key = self.key(chain.key);
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

    fn stage_charge(term: Option<u16>, copy_key: usize) -> usize {
        let links = if term.is_some() {
            2 * size_of::<u16>()
        } else {
            copy_key + size_of::<TermChain>() + size_of::<(u64, u16)>()
        };
        size_of::<ColdRow>() + links
    }

    fn link_row(&mut self, owner: u16, reference: u16, hash: u64, term: Option<u16>, key: TermKey) {
        // Both arrays are bounded by MAX_EDIT_ROWS, below the sentinel.
        let row = self.rows.len() as u16;
        self.rows.push(ColdRow {
            owner,
            reference,
            next: END_OF_CHAIN,
        });
        if let Some(id) = term {
            let chain = &mut self.terms[usize::from(id)];
            self.rows[usize::from(chain.last)].next = row;
            chain.last = row;
        } else {
            let same_hash = self.lookup.insert(hash, self.terms.len() as u16);
            self.terms.push(TermChain {
                key,
                first: row,
                last: row,
                same_hash,
            });
        }
    }

    pub(super) fn push(
        &mut self,
        row: RowView<'_>,
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
            let chain = &self.terms[usize::from(id)];
            let last = &self.rows[usize::from(chain.last)];
            if self.packet.owners[usize::from(last.owner)] >= row.owner {
                return Err(corrupt("duplicate or unordered cold posting key"));
            }
        }
        let key = TermKey {
            start: u32::try_from(self.keys.len()).map_err(|_| IndexError::ResourceLimited)?,
            length: u16::try_from(row.term.len()).map_err(|_| IndexError::ResourceLimited)?,
        };
        // The dictionary writes mirror the general writer. Pay for only
        // actual term/row staging here; the encoder charges output separately.
        measured_copy(
            Self::stage_charge(term, row.term.len()) + 2 * layout.dictionary_bytes,
            budget,
        )?;
        let owner = self.packet.owner_id(row.owner)?;
        let reference = self.packet.reference_id(reference)?;
        if term.is_none() {
            self.keys.extend_from_slice(row.term);
        } else {
            self.repeated_term_bytes += row.term.len();
        }
        self.link_row(owner, reference, hash, term, key);
        self.encoded_end += layout.row_len;
        self.packet.count += 1;
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
            budget.charge(WorkUsage {
                bytes: width(key_bytes + self.terms.len() * size_of::<u16>())?,
                ..WorkUsage::default()
            })?;
            let count = u16::try_from(self.terms.len()).map_err(|_| IndexError::ResourceLimited)?;
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
            .map_err(|_| IndexError::ResourceLimited)?;
        ordered.resize(EDIT_HEADER, 0);
        // Run zero can be the final seekable stream. Subsequent phase-zero
        // runs are consumed only by the merge and can encode one key per chain.
        let group_terms = phase == 0 && run > 0 && self.repeated_term_bytes > 0;
        let encoded_end = self.encoded_end
            - if group_terms {
                self.repeated_term_bytes
            } else {
                0
            };
        measured_copy(encoded_end - EDIT_HEADER, budget)?;
        let mut first = None;
        let mut last = None;
        let mut prefix = self.packet.offsets.iter().peekable();
        for &id in &self.order {
            budget.checkpoint()?;
            let chain = &self.terms[usize::from(id)];
            let key = self.key(chain.key);
            let first_suffix_owner =
                self.packet.owners[usize::from(self.rows[usize::from(chain.first)].owner)];
            // The cold input is owner-major and the prefix precedes the suffix.
            // Once the prefix is past this term, its next key cannot precede
            // any of this term's remaining owners.
            while let Some(&&offset) = prefix.peek() {
                let prior = self.packet.fence(offset);
                let term_order = prior.0.cmp(key);
                let compared = prior.0.len().min(key.len())
                    + usize::from(term_order == Ordering::Equal) * size_of::<[u8; 32]>();
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
                ordered.extend_from_slice(&self.packet.bytes[offset.start..offset.end]);
                let _first = first.get_or_insert(prior);
                let _offset = prefix.next();
            }
            let mut row_id = chain.first;
            let literal_offset = ordered.len();
            while row_id != END_OF_CHAIN {
                let row = &self.rows[usize::from(row_id)];
                let fence = (key, self.packet.owners[usize::from(row.owner)]);
                if group_terms && row_id != chain.first {
                    let distance = ordered.len() - literal_offset;
                    let distance = u16::try_from(distance)
                        .ok()
                        .filter(|&n| n < REPEATED_TERM)
                        .ok_or(IndexError::ResourceLimited)?;
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
            ordered.extend_from_slice(&self.packet.bytes[offset.start..offset.end]);
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
            ordered[..8].copy_from_slice(GROUPED_EDIT_MAGIC);
        }
        spool.insert_posting_packet(
            phase,
            run,
            0,
            PostingPacketFences {
                first: first.ok_or_else(|| corrupt("cold run has no first key"))?,
                last: last.ok_or_else(|| corrupt("cold run has no last key"))?,
            },
            &ordered,
            budget,
        )
    }
}
