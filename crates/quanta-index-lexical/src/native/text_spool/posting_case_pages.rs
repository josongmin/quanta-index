//! Bounded case-paired posting pages.
//!
//! Each surface advances in primary-key order. Pairing controls scheduling.
//! Map equality requires the provider's exact base/edit comparison and
//! independent original-stream validation.

use super::{MAX_TERM_KEY_BYTES, PAGE, TermPage, TextSpool, check_term_key, sql, term_page};
use crate::native::text_memory::PreparedKey;
use quanta_index_core::index::{IndexError, WorkBudget, WorkUsage};
const SENSITIVE_TERMS_SQL: &str = "WITH RECURSIVE next(term,n) AS (
    SELECT (SELECT term FROM posting WHERE term>?1 AND term>=X'00' AND term<X'01' ORDER BY term LIMIT 1),1
    UNION ALL
    SELECT (SELECT term FROM posting WHERE term>next.term AND term<X'01' ORDER BY term LIMIT 1),n+1
    FROM next WHERE term IS NOT NULL AND n<64
  ) SELECT term FROM next WHERE term IS NOT NULL ORDER BY n";
const FOLDED_TERMS_SQL: &str = "WITH RECURSIVE next(term,n) AS (
    SELECT (SELECT term FROM posting WHERE term>?1 AND term>=X'01' AND term<X'02' ORDER BY term LIMIT 1),1
    UNION ALL
    SELECT (SELECT term FROM posting WHERE term>next.term AND term<X'02' ORDER BY term LIMIT 1),n+1
    FROM next WHERE term IS NOT NULL AND n<64
  ) SELECT term FROM next WHERE term IS NOT NULL ORDER BY n";
const GLOBAL_FIRST_SQL: &str = "SELECT term FROM posting ORDER BY term LIMIT 1";
const GLOBAL_LAST_SQL: &str = "SELECT term FROM posting ORDER BY term DESC LIMIT 1";

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
        let db = spool.db.scoped(budget)?;
        let fetched = term_page(
            &db,
            if self.surface == 0 {
                SENSITIVE_TERMS_SQL
            } else {
                FOLDED_TERMS_SQL
            },
            self.after.as_ref().map(PreparedKey::as_slice),
            budget,
        )?;
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

fn validate_global_bound(
    spool: &TextSpool,
    query: &str,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 4096,
        ..WorkUsage::default()
    })?;
    let db = spool.db.scoped(budget)?;
    let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    if let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let term = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(0).map_err(|error| sql(&error))?,
            usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        )?;
        check_term_key(term)?;
    }
    budget.checkpoint()
}

pub(in crate::native) struct PostingCasePages {
    sensitive: CaseCursor,
    folded: CaseCursor,
    initialized: bool,
}

impl PostingCasePages {
    pub(in crate::native) fn new() -> Self {
        Self {
            sensitive: CaseCursor::new(0),
            folded: CaseCursor::new(1),
            initialized: false,
        }
    }

    pub(in crate::native) fn next(
        &mut self,
        spool: &TextSpool,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        if !self.initialized {
            validate_global_bound(spool, GLOBAL_FIRST_SQL, budget)?;
            validate_global_bound(spool, GLOBAL_LAST_SQL, budget)?;
            self.initialized = true;
        }
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
            if choice != 1 {
                output.push(
                    self.sensitive.peek().ok_or(IndexError::ResourceLimited)?,
                    budget,
                )?;
                self.sensitive.consume()?;
                saw_sensitive = true;
            }
            if choice != 0 {
                output.push(
                    self.folded.peek().ok_or(IndexError::ResourceLimited)?,
                    budget,
                )?;
                self.folded.consume()?;
                saw_folded = true;
            }
        }
        output
            .keys
            .sort_unstable_by(|left, right| left.as_slice().cmp(right.as_slice()));
        output.case_ranges = saw_sensitive && saw_folded;
        Ok(output)
    }
}
