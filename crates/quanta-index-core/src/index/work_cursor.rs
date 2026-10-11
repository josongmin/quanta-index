//! Authenticated replay of sealed publication work actions.
use std::collections::VecDeque;

use super::ports::{ImmutableObjectPort, PersistentMapPort, WorkActionCursor};
use super::{
    IndexError, ObjectRef, WorkAction, WorkActionDigest, WorkActionSet, WorkBudget, WorkUsage,
};

pub struct WorkActionManifestCursor<'a> {
    set: WorkActionSet,
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
    page: VecDeque<(Vec<u8>, ObjectRef)>,
    after: Option<Vec<u8>>,
    more: bool,
    finished: bool,
    digest: WorkActionDigest,
}

impl<'a> WorkActionManifestCursor<'a> {
    #[must_use]
    pub fn new(
        set: WorkActionSet,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Self {
        Self {
            set,
            maps,
            objects,
            page: VecDeque::new(),
            after: None,
            more: true,
            finished: false,
            digest: WorkActionDigest::default(),
        }
    }
}

impl WorkActionCursor for WorkActionManifestCursor<'_> {
    fn next_action(&mut self, budget: &mut WorkBudget) -> Result<Option<WorkAction>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.page.is_empty() && self.more {
            let page = self
                .maps
                .scan(self.set.root, self.after.as_deref(), 128, budget)?;
            if page.entries.len() > 128 || (page.has_more && page.entries.is_empty()) {
                return Err(IndexError::BackendViolation(
                    "invalid bounded work manifest page".into(),
                ));
            }
            self.more = page.has_more;
            self.page = page.entries.into();
        }
        let Some((key, reference)) = self.page.pop_front() else {
            if self.digest.count() != self.set.count || self.digest.finish() != self.set.digest {
                return Err(IndexError::CorruptObject(
                    "work manifest count or digest mismatch".into(),
                ));
            }
            self.finished = true;
            return Ok(None);
        };
        if self.after.as_ref().is_some_and(|prior| prior >= &key) {
            return Err(IndexError::CorruptObject(
                "work manifest page order mismatch".into(),
            ));
        }
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let stored = self.objects.get(reference, budget)?;
        if stored.registration.reference != reference {
            return Err(IndexError::CorruptObject(
                "work action reference mismatch".into(),
            ));
        }
        let action: WorkAction = serde_json::from_slice(&stored.payload)
            .map_err(|error| IndexError::CorruptObject(format!("work action: {error}")))?;
        let canonical = serde_json::to_vec(&action)
            .map_err(|error| IndexError::CorruptObject(format!("work action: {error}")))?;
        if action.key()? != key
            || stored.registration.children != action.children()
            || canonical != stored.payload
        {
            return Err(IndexError::CorruptObject(
                "work action key, encoding or children mismatch".into(),
            ));
        }
        self.digest.push(&action)?;
        if self.digest.count() > self.set.count {
            return Err(IndexError::CorruptObject(
                "work manifest exceeds declared count".into(),
            ));
        }
        self.after = Some(key);
        Ok(Some(action))
    }
}
