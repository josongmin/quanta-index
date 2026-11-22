//! Bounded case-paired posting pages.
//!
//! Each surface advances in primary-key order. Pairing controls scheduling.
//! Map equality requires the provider's exact base/edit comparison and
//! independent original-stream validation.

use super::{PAGE, PostingCursor, PostingPacketCache, TermPage, TextSpool, check_term_key};
use crate::native::text_memory::PreparedKey;
use quanta_index_core::index::{IndexError, WorkBudget};

struct CaseCursor {
    surface: u8,
    page: TermPage,
    next: usize,
    after: Option<PreparedKey>,
    eof: bool,
}

impl CaseCursor {
    fn new(surface: u8) -> Self {
        Self {
            surface,
            page: TermPage::new(),
            next: 0,
            after: None,
            eof: false,
        }
    }

    fn fill(&mut self, spool: &TextSpool, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.next < self.page.len() || self.eof {
            return Ok(());
        }
        budget.checkpoint()?;
        let fetched = spool.packed_term_page(
            self.after.as_ref().map(PreparedKey::as_slice),
            Some(self.surface),
            budget,
        )?;
        // The packed reader already sealed this private page. It stays owned
        // by the cursor until its entries are copied into a sealed pair.
        let mut previous = self.after.as_ref().map(PreparedKey::as_slice);
        for key in fetched.iter() {
            let term = key.as_slice();
            check_term_key(term)?;
            if term.first() != Some(&self.surface) || previous.is_some_and(|prior| prior >= term) {
                return Err(IndexError::CorruptObject(
                    "posting case term page crossed its ordered surface".into(),
                ));
            }
            previous = Some(term);
        }
        self.eof = fetched.is_empty();
        self.page = fetched;
        self.next = 0;
        Ok(())
    }

    fn peek(&self) -> Option<&[u8]> {
        self.page.get(self.next).map(PreparedKey::as_slice)
    }

    fn consume(&mut self) -> Result<(), IndexError> {
        if self.peek().is_none() {
            return Err(IndexError::BackendViolation(
                "posting case cursor consumed after EOF".into(),
            ));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if self.next == self.page.len() {
            self.after = self.page.keys.pop();
            self.page = TermPage::new();
            self.next = 0;
        }
        Ok(())
    }
}

pub(in crate::native) struct PostingCasePages {
    sensitive: CaseCursor,
    folded: CaseCursor,
    packets: PostingPacketCache,
}

impl PostingCasePages {
    pub(in crate::native) fn new() -> Self {
        Self {
            sensitive: CaseCursor::new(0),
            folded: CaseCursor::new(1),
            packets: PostingPacketCache::default(),
        }
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "Exclusive access keeps one posting cursor alive per admitted page-cache working set"
    )]
    pub(in crate::native) fn posting_cursor<'a>(
        &'a mut self,
        spool: &'a TextSpool,
        terms: TermPage,
    ) -> PostingCursor<'a> {
        spool.posting_cursor_with_packets(terms, Some(&self.packets))
    }

    pub(in crate::native) fn next(
        &mut self,
        spool: &TextSpool,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        spool.check_posting_usable()?;
        budget.checkpoint()?;
        spool.postings.check_page_source(spool)?;
        let mut output = TermPage::new();
        let mut saw_sensitive = false;
        let mut saw_folded = false;
        let pair_limit = PAGE.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
        while output.len() < PAGE {
            self.sensitive.fill(spool, budget)?;
            self.folded.fill(spool, budget)?;
            let choice = match (self.sensitive.peek(), self.folded.peek()) {
                (None, None) => break,
                (Some(_), None) => 0_u8,
                (None, Some(_)) => 1_u8,
                (Some(left), Some(right)) => match left
                    .get(1..)
                    .ok_or(IndexError::ResourceLimited)?
                    .cmp(right.get(1..).ok_or(IndexError::ResourceLimited)?)
                {
                    std::cmp::Ordering::Less => 0,
                    std::cmp::Ordering::Equal => 2,
                    std::cmp::Ordering::Greater => 1,
                },
            };
            if choice == 2 && output.len() == pair_limit {
                break;
            }
            if output.is_empty() {
                let source = spool.postings.page_source(spool, budget)?;
                output = TermPage::counted(source, budget)?;
            }
            if choice != 1 {
                output.push_from(&self.sensitive.page, self.sensitive.next, budget)?;
                self.sensitive.consume()?;
                saw_sensitive = true;
            }
            if choice != 0 {
                output.push_from(&self.folded.page, self.folded.next, budget)?;
                self.folded.consume()?;
                saw_folded = true;
            }
        }
        if !output.is_empty() {
            output.sort_counted()?;
            output.case_ranges = saw_sensitive && saw_folded;
            output.seal_counts(budget)?;
        }
        Ok(output)
    }
}
