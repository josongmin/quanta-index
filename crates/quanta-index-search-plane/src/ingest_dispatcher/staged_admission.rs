//! Process-local admission for decoded staged publications.

use std::collections::HashSet;
use std::sync::{Mutex, TryLockError};

use quanta_index_contract::{RepoId, SOURCE_PUBLICATION_UPLOAD_MAX_BYTES, SearchPlaneErrorCodeV2};
use quanta_index_core::CoreError;
use quanta_index_ipc::SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS;

// Admission charges encoded body bytes plus CBOR-preflight retained text/header
// storage, using the existing ingress limits of 512 MiB each. The sum is a
// nominal 1 GiB admission ledger, not a total RSS or allocator heap bound.
// Native WorkBytes has a separate, unchanged 512 MiB ledger.
const MAX_DECODED_BYTES: u64 = SOURCE_PUBLICATION_UPLOAD_MAX_BYTES * 2;

#[derive(Default)]
struct State {
    repos: HashSet<RepoId>,
    decoded_bytes: u64,
    poisoned: bool,
}

#[derive(Default)]
pub(super) struct StagedAdmission {
    state: Mutex<State>,
}

fn busy(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogBusy,
        message: message.into(),
    }
}

impl StagedAdmission {
    pub(super) fn begin(&self, repo: RepoId) -> Result<StagedGuard<'_>, CoreError> {
        let mut state = self.state.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => busy("staged source admission busy"),
            TryLockError::Poisoned(_) => {
                CoreError::Storage("staged source admission poisoned".into())
            }
        })?;
        if state.poisoned {
            return Err(CoreError::Storage(
                "staged source admission poisoned".into(),
            ));
        }
        if state.repos.contains(&repo) {
            return Err(busy("staged source publication already active for repo"));
        }
        if state.repos.len() >= SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS {
            return Err(busy("staged source publication slot quota exceeded"));
        }
        let _inserted = state.repos.insert(repo.clone());
        Ok(StagedGuard {
            owner: self,
            repo,
            decoded_bytes: 0,
        })
    }
}

pub(super) struct StagedGuard<'a> {
    owner: &'a StagedAdmission,
    repo: RepoId,
    decoded_bytes: u64,
}

impl StagedGuard<'_> {
    pub(super) fn require_reserved(&self) -> Result<(), CoreError> {
        let state = self.owner.state.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => busy("staged source admission busy"),
            TryLockError::Poisoned(_) => {
                CoreError::Storage("staged source admission poisoned".into())
            }
        })?;
        if state.poisoned || !state.repos.contains(&self.repo) {
            return Err(CoreError::Storage(
                "staged source admission lost custody".into(),
            ));
        }
        if self.decoded_bytes == 0 || state.decoded_bytes < self.decoded_bytes {
            return Err(CoreError::InvalidContract(
                "source upload load returned without decoded-byte reservation".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn reserve(&mut self, decoded_bytes: u64) -> Result<(), CoreError> {
        if self.decoded_bytes != 0 || decoded_bytes == 0 || decoded_bytes > MAX_DECODED_BYTES {
            return Err(CoreError::InvalidContract(
                "invalid staged source decoded-byte reservation".into(),
            ));
        }
        let mut state = self.owner.state.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => busy("staged source admission busy"),
            TryLockError::Poisoned(_) => {
                CoreError::Storage("staged source admission poisoned".into())
            }
        })?;
        if state.poisoned || !state.repos.contains(&self.repo) {
            return Err(CoreError::Storage(
                "staged source admission lost custody".into(),
            ));
        }
        let total = state
            .decoded_bytes
            .checked_add(decoded_bytes)
            .ok_or_else(|| busy("staged source decoded-byte quota exceeded"))?;
        if total > MAX_DECODED_BYTES {
            return Err(busy("staged source decoded-byte quota exceeded"));
        }
        state.decoded_bytes = total;
        self.decoded_bytes = decoded_bytes;
        Ok(())
    }
}

impl Drop for StagedGuard<'_> {
    fn drop(&mut self) {
        // A poisoned mutex remains a refusal for future admissions. Still
        // release this guard's own counters when the poisoned guard is usable.
        let mut state = match self.owner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !state.repos.remove(&self.repo) {
            state.poisoned = true;
        }
        match state.decoded_bytes.checked_sub(self.decoded_bytes) {
            Some(remaining) => state.decoded_bytes = remaining,
            None => state.poisoned = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> RepoId {
        RepoId::new(name).expect("fixture repo")
    }

    #[test]
    fn two_repos_share_cap_and_release_on_drop() {
        let admission = StagedAdmission::default();
        let mut first = admission.begin(repo("first")).expect("first repo");
        first.reserve(MAX_DECODED_BYTES / 2).expect("first half");
        assert!(matches!(
            admission.begin(repo("first")),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogBusy,
                ..
            })
        ));
        let mut second = admission.begin(repo("second")).expect("other repo");
        assert!(matches!(
            second.reserve(MAX_DECODED_BYTES / 2 + 1),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogBusy,
                ..
            })
        ));
        second
            .reserve(MAX_DECODED_BYTES / 2)
            .expect("remaining half");
        drop(first);
        drop(second);
        let mut third = admission.begin(repo("first")).expect("released repo");
        third.reserve(MAX_DECODED_BYTES).expect("released bytes");
    }

    #[test]
    fn slot_quota_and_missing_reservation_fail_closed() {
        let admission = StagedAdmission::default();
        let mut guards = Vec::new();
        for index in 0..SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS {
            let guard = admission
                .begin(repo(&format!("repo-{index}")))
                .expect("staging slot");
            assert!(matches!(
                guard.require_reserved(),
                Err(CoreError::InvalidContract(_))
            ));
            guards.push(guard);
        }
        assert!(matches!(
            admission.begin(repo("overflow")),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogBusy,
                ..
            })
        ));
        drop(guards.pop());
        let mut replacement = admission.begin(repo("overflow")).expect("released slot");
        replacement.reserve(1).expect("positive reservation");
        replacement.require_reserved().expect("held reservation");
    }

    #[test]
    fn cancellation_failure_and_unwind_release_slots_and_bytes() {
        fn fail_after_reserve(
            admission: &StagedAdmission,
            error: CoreError,
        ) -> Result<(), CoreError> {
            let mut guard = admission.begin(repo("repo"))?;
            guard.reserve(16)?;
            Err(error)
        }

        let admission = StagedAdmission::default();
        for error in [
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                message: "fixture cancellation".into(),
            },
            CoreError::InvalidContract("fixture dispatch failure".into()),
        ] {
            assert_eq!(fail_after_reserve(&admission, error.clone()), Err(error));
            let mut next = admission.begin(repo("repo")).expect("error released slot");
            next.reserve(MAX_DECODED_BYTES)
                .expect("error released bytes");
        }

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut guard = admission.begin(repo("repo")).expect("unwind slot");
            guard.reserve(16).expect("unwind bytes");
            panic!("fixture unwind");
        }));
        assert!(unwind.is_err());
        let mut next = admission.begin(repo("repo")).expect("unwind released slot");
        next.reserve(MAX_DECODED_BYTES)
            .expect("unwind released bytes");
    }
}
