//! Opt-in, process-scoped response loss at the authenticated client transport.

use std::path::Path;
use std::sync::Mutex;

#[derive(Clone, Copy, Default)]
struct StateV1 {
    selector: Option<fn(&Path, &[u8]) -> bool>,
    selected: bool,
    fired: bool,
    generation: u64,
}

static STATE_V1: Mutex<StateV1> = Mutex::new(StateV1 {
    selector: None,
    selected: false,
    fired: false,
    generation: 0,
});

/// Arm one exact request before dispatch. The selector sees the socket and encoded frame.
#[must_use]
pub fn try_arm_v1(selector: fn(&Path, &[u8]) -> bool) -> bool {
    let mut state = STATE_V1.lock().expect("response-loss state");
    if state.selector.is_some() || state.selected || state.fired {
        return false;
    }
    state.generation = state.generation.wrapping_add(1);
    state.selector = Some(selector);
    true
}

pub(crate) fn take_selected_v1(socket: &Path, frame: &[u8]) -> bool {
    let (selector, generation) = {
        let state = STATE_V1.lock().expect("response-loss state");
        if state.selected {
            return false;
        }
        (state.selector, state.generation)
    };
    if !selector.is_some_and(|selector| selector(socket, frame)) {
        return false;
    }
    let mut state = STATE_V1.lock().expect("response-loss state");
    if state.generation != generation || state.selector.is_none() || state.selected {
        return false;
    }
    state.selector = None;
    state.selected = true;
    true
}

pub(crate) fn mark_fired_v1() {
    let mut state = STATE_V1.lock().expect("response-loss state");
    assert!(state.selected && !state.fired, "unselected response loss");
    state.fired = true;
}

pub(crate) fn abort_selected_v1() {
    let mut state = STATE_V1.lock().expect("response-loss state");
    assert!(
        state.selected && !state.fired,
        "unselected response-loss abort"
    );
    state.selected = false;
    state.generation = state.generation.wrapping_add(1);
}

#[must_use]
pub fn fired_v1() -> bool {
    STATE_V1.lock().expect("response-loss state").fired
}

/// Release an arm after a completed test. A concurrent selected send retains custody.
#[must_use]
pub fn disarm_v1() -> bool {
    let mut state = STATE_V1.lock().expect("response-loss state");
    if state.selected && !state.fired {
        return false;
    }
    let generation = state.generation.wrapping_add(1);
    *state = StateV1 {
        generation,
        ..StateV1::default()
    };
    true
}
