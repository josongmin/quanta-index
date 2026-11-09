//! Process-scoped test counter for Native UDS send attempts.
use std::sync::Mutex;

#[derive(Clone, Copy)]
struct SendObservationV1 {
    armed: bool,
    count: u64,
    overflow: bool,
}

impl SendObservationV1 {
    fn checked_count(self) -> u64 {
        assert!(self.armed, "Native send observation is not armed");
        assert!(!self.overflow, "Native send observation overflowed");
        self.count
    }
}

static STATE_V1: Mutex<SendObservationV1> = Mutex::new(SendObservationV1 {
    armed: false,
    count: 0,
    overflow: false,
});

/// Arm only after all earlier synchronous requests have completed.
pub fn try_arm_v1() -> bool {
    let mut state = STATE_V1.lock().expect("Native send observer lock");
    if state.armed {
        return false;
    }
    *state = SendObservationV1 {
        armed: true,
        count: 0,
        overflow: false,
    };
    true
}

/// Count every send attempt in the process, including unrelated requests.
pub(crate) fn observe_v1() {
    let mut state = STATE_V1.lock().expect("Native send observer lock");
    if state.armed {
        if let Some(next) = state.count.checked_add(1) {
            state.count = next;
        } else {
            state.overflow = true;
        }
    }
}

pub fn snapshot_v1() -> u64 {
    let observation = *STATE_V1.lock().expect("Native send observer lock");
    observation.checked_count()
}

pub fn disarm_v1() -> u64 {
    let observation = {
        let mut state = STATE_V1.lock().expect("Native send observer lock");
        let observation = *state;
        state.armed = false;
        observation
    };
    observation.checked_count()
}

/// Release an owned observation during unwinding without replacing the first failure.
pub fn abandon_v1() {
    let mut state = STATE_V1
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.armed = false;
}

#[cfg(test)]
mod tests {
    use super::{disarm_v1, observe_v1, snapshot_v1, try_arm_v1};

    #[test]
    fn assertion_panics_leave_observer_reusable() {
        let operations: [fn() -> u64; 2] = [snapshot_v1, disarm_v1];
        for operation in operations {
            let panic = std::panic::catch_unwind(operation)
                .expect_err("an unarmed observer must reject the operation");
            assert_eq!(
                panic.downcast_ref::<&str>(),
                Some(&"Native send observation is not armed")
            );
            assert!(try_arm_v1());
            assert_eq!(snapshot_v1(), 0);
            observe_v1();
            assert_eq!(snapshot_v1(), 1);
            assert_eq!(disarm_v1(), 1);
        }
    }
}
