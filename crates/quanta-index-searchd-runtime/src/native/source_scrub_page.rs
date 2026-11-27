//! Bounded maintenance of the Source owners in one exact Native view.

use quanta_index_contract::index::{MapRoot, OwnerKey, ViewRef};
use quanta_index_core::index::{
    IndexError, WorkBudget, effort::ReservationGuard, owner_from_map_key, owner_map_key,
    ports::PersistentMapPort as _,
};

use super::NativeIndexMaintenance;

/// An opaque position in one authenticated Source-owner map.
///
/// The caller may retain this value across maintenance calls. It has no wire
/// encoding or durable progress claim; a process restart must begin at None.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSourceScrubCursor {
    view: ViewRef,
    owners: MapRoot,
    after: OwnerKey,
}

/// One complete, bounded page. `next == None` means this immutable owner map
/// was exhausted by this call; it does not claim a persisted whole-view audit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSourceScrubPage {
    pub checked: u32,
    pub damaged: u32,
    pub next: Option<NativeSourceScrubCursor>,
    /// The selected view was fenced before this page began.
    pub previously_quarantined: bool,
}

impl NativeIndexMaintenance {
    /// Explicitly create restart-safe progress for one retained exact view.
    /// Repeated starts refuse instead of clearing an unfinished or completed
    /// position. The page method resumes only an existing request.
    pub fn start_source_body_scrub_progress(
        &self,
        view: ViewRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let reservation = ReservationGuard::try_acquire(
            std::sync::Arc::clone(&self.maintenance_scheduler),
            self.maintenance_need,
        )
        .ok_or_else(|| self.maintenance_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let _physical = self.io.enter_maintenance_scope()?;
        self.catalog.start_source_body_scrub_progress(view, budget)
    }

    /// Run and commit one durable page. A failure returns no page; retrying
    /// reads the last committed position, even after process restart.
    pub fn scrub_source_body_progress_page(
        &self,
        view: ViewRef,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<Option<NativeSourceScrubPage>, IndexError> {
        if !(1..=128).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "Source scrub page limit must be 1..=128".into(),
            ));
        }
        budget.checkpoint()?;
        let reservation = ReservationGuard::try_acquire(
            std::sync::Arc::clone(&self.maintenance_scheduler),
            self.maintenance_need,
        )
        .ok_or_else(|| self.maintenance_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let _physical = self.io.enter_maintenance_scope()?;
        self.catalog
            .with_source_body_scrub_progress_page(view, budget, |owners, after, budget| {
                let cursor = after.map(|after| NativeSourceScrubCursor {
                    view,
                    owners,
                    after,
                });
                let page = self.scrub_source_body_page_admitted(view, cursor, limit, budget)?;
                Ok((page, page.next.map(|cursor| cursor.after)))
            })
    }

    fn fence_corrupt_source_owner_map<T>(
        &self,
        view: ViewRef,
        owners: MapRoot,
        result: Result<T, IndexError>,
        budget: &mut WorkBudget,
    ) -> Result<T, IndexError> {
        if matches!(&result, Err(IndexError::CorruptObject(_))) {
            if let Some(root) = owners.0 {
                let _incident = self.catalog.quarantine_view_object(view, root, budget)?;
            }
        }
        result
    }

    /// Re-prove at most `limit` Source records and their bodies in key order.
    /// Each confirmed defect is fenced before the page can be returned. A
    /// failed call returns no continuation; retry its input cursor. A later
    /// page may proceed on an already quarantined view to find other defects.
    pub fn scrub_source_body_page(
        &self,
        view: ViewRef,
        after: Option<NativeSourceScrubCursor>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<NativeSourceScrubPage, IndexError> {
        if !(1..=128).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "Source scrub page limit must be 1..=128".into(),
            ));
        }
        if after.is_some_and(|cursor| cursor.view != view) {
            return Err(IndexError::InvalidInput(
                "Source scrub cursor differs from selected view or Source cut".into(),
            ));
        }
        budget.checkpoint()?;
        let reservation = ReservationGuard::try_acquire(
            std::sync::Arc::clone(&self.maintenance_scheduler),
            self.maintenance_need,
        )
        .ok_or_else(|| self.maintenance_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let _physical = self.io.enter_maintenance_scope()?;
        self.scrub_source_body_page_admitted(view, after, limit, budget)
    }

    fn scrub_source_body_page_admitted(
        &self,
        view: ViewRef,
        after: Option<NativeSourceScrubCursor>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<NativeSourceScrubPage, IndexError> {
        let _resumed_incident = self.catalog.resume_source_body_quarantine(view, budget)?;
        self.kernel.with_maintenance_read_session(|session, lease| {
            self.catalog.with_source_body_scrub_pin(
                session,
                lease,
                view,
                budget,
                |owners, previously_quarantined, budget| {
                    if after.is_some_and(|cursor| cursor.view != view || cursor.owners != owners) {
                        return Err(IndexError::InvalidInput(
                            "Source scrub cursor differs from selected view or Source cut".into(),
                        ));
                    }
                    let after_key = after.map(|cursor| owner_map_key(cursor.after));
                    let scanned = self.objects.scan(
                        owners,
                        after_key.as_ref().map(|key| key.as_slice()),
                        limit,
                        budget,
                    );
                    let page =
                        self.fence_corrupt_source_owner_map(view, owners, scanned, budget)?;
                    if page.entries.len()
                        > usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)?
                        || (page.entries.is_empty() && page.has_more)
                    {
                        return Err(IndexError::BackendViolation(
                            "Source scrub scan exceeded its page or omitted a row".into(),
                        ));
                    }
                    let mut previous = after_key.as_ref().map(|key| key.as_slice());
                    let mut last = None;
                    let mut checked = 0_u32;
                    let mut damaged = 0_u32;
                    for (key, selected) in &page.entries {
                        budget.checkpoint()?;
                        if previous.is_some_and(|previous| key.as_slice() <= previous) {
                            return Err(IndexError::BackendViolation(
                                "Source scrub scan is not strictly ordered".into(),
                            ));
                        }
                        let owner = self.fence_corrupt_source_owner_map(
                            view,
                            owners,
                            owner_from_map_key(key),
                            budget,
                        )?;
                        let selected = self.fence_corrupt_source_owner_map(
                            view,
                            owners,
                            selected.object_ref(),
                            budget,
                        )?;
                        let (object, intact) = self.check_source_body_ref(selected, budget)?;
                        if !intact {
                            let _incident =
                                self.catalog.quarantine_view_object(view, object, budget)?;
                            damaged = damaged.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        }
                        checked = checked.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        previous = Some(key.as_slice());
                        last = Some(owner);
                    }
                    let next = if page.has_more {
                        Some(NativeSourceScrubCursor {
                            view,
                            owners,
                            after: last.ok_or_else(|| {
                                IndexError::BackendViolation(
                                    "Source scrub continuation is empty".into(),
                                )
                            })?,
                        })
                    } else {
                        None
                    };
                    Ok(NativeSourceScrubPage {
                        checked,
                        damaged,
                        next,
                        previously_quarantined,
                    })
                },
            )
        })
    }
}
