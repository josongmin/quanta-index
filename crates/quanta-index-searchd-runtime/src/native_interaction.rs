//! Bounded foreground publisher custody for Native Auto.
//!
//! A one-shot UDS connection cannot prove that an IDE remains connected.
//! Explicit End invalidates immediately; an abrupt producer exit becomes
//! Unknown when its finite lease expires. Only an actual activity event resets
//! idle time. Renewal, Begin, query traffic, and worker idleness do not.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use quanta_index_contract::native_protocol::NativeInteractionPublisherId;
use quanta_index_core::index::{IndexError, effort::UnknownReason};

use crate::native_auto_signals::{NativeInteractionActivity, NativeInteractionActivityPort};

const MIN_PUBLISHER_WINDOW: Duration = Duration::from_millis(1);
const MAX_PUBLISHER_WINDOW: Duration = Duration::from_secs(60);

fn validate_window(window: Duration) -> Result<(), IndexError> {
    if !(MIN_PUBLISHER_WINDOW..=MAX_PUBLISHER_WINDOW).contains(&window) {
        return Err(IndexError::InvalidInput(
            "native activity publisher window must be 1..=60000 ms".into(),
        ));
    }
    Ok(())
}

pub(crate) trait InteractionClock: Send + Sync {
    fn now(&self) -> Instant;
}

struct MonotonicClock;

impl InteractionClock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Clone, Copy)]
struct PublisherLease {
    uid: u32,
    id: NativeInteractionPublisherId,
    window: Duration,
    valid_until: Instant,
    last_command_at: Instant,
    last_activity_at: Option<Instant>,
    last_sequence: u64,
    ended: bool,
}

#[derive(Default)]
struct RecorderState {
    /// Never reused in this process, including after a lease ends or expires.
    epoch: u64,
    current: Option<PublisherLease>,
}

#[derive(Clone, Copy)]
enum PublisherCommand {
    Activity,
    Renew,
    End,
}

/// One process-wide foreground publisher session.
///
/// The caller creates a fresh server-random identity and supplies the peer UID
/// from kernel UDS credentials, never from a request field. One active session
/// is admitted at a time. The producer must cover the intended foreground
/// activity source; this recorder cannot attest to activity in other UIs.
pub struct NativeInteractionRecorder {
    clock: Arc<dyn InteractionClock>,
    state: Mutex<RecorderState>,
}

impl Default for NativeInteractionRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::native_overlay::OverlayPublisherAuthPort for NativeInteractionRecorder {
    fn authenticate(
        &self,
        uid: u32,
        publisher: NativeInteractionPublisherId,
    ) -> Result<(), IndexError> {
        self.validate_live(uid, publisher)
    }
}

impl NativeInteractionRecorder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: Arc::new(MonotonicClock),
            state: Mutex::new(RecorderState::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(clock: Arc<dyn InteractionClock>) -> Self {
        Self {
            clock,
            state: Mutex::new(RecorderState::default()),
        }
    }

    /// Begin a finite session. This does not create an activity observation.
    pub fn begin(
        &self,
        uid: u32,
        id: NativeInteractionPublisherId,
        window: Duration,
    ) -> Result<(), IndexError> {
        validate_window(window)?;
        if id.0 == [0; 32] {
            return Err(IndexError::InvalidInput(
                "native activity publisher identity must be nonzero".into(),
            ));
        }
        let mut state = self.state.lock().map_err(|error| {
            IndexError::Storage(format!("native activity recorder lock poisoned: {error}"))
        })?;
        // Clock receipt is ordered with all other publisher commands.
        let now = self.clock.now();
        if let Some(prior) = state.current {
            if now < prior.last_command_at {
                return Err(IndexError::Storage(
                    "native activity monotonic clock regressed".into(),
                ));
            }
            if prior.id == id || (!prior.ended && now < prior.valid_until) {
                return Err(IndexError::Conflict);
            }
        }
        let valid_until = now.checked_add(window).ok_or_else(|| {
            IndexError::InvalidInput("native activity publisher expiry overflow".into())
        })?;
        let next_epoch = state
            .epoch
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        state.epoch = next_epoch;
        state.current = Some(PublisherLease {
            uid,
            id,
            window,
            valid_until,
            last_command_at: now,
            last_activity_at: None,
            last_sequence: 0,
            ended: false,
        });
        drop(state);
        Ok(())
    }

    /// Accept an actual user or IDE interaction, then reset idle time.
    pub fn activity(
        &self,
        uid: u32,
        id: NativeInteractionPublisherId,
        sequence: u64,
    ) -> Result<(), IndexError> {
        self.command(uid, id, sequence, PublisherCommand::Activity)
    }

    /// Keep an existing session live without resetting its activity clock.
    pub fn renew(
        &self,
        uid: u32,
        id: NativeInteractionPublisherId,
        sequence: u64,
    ) -> Result<(), IndexError> {
        self.command(uid, id, sequence, PublisherCommand::Renew)
    }

    /// Immediately fence an existing publisher and make activity Unknown.
    pub fn end(
        &self,
        uid: u32,
        id: NativeInteractionPublisherId,
        sequence: u64,
    ) -> Result<(), IndexError> {
        self.command(uid, id, sequence, PublisherCommand::End)
    }

    /// Authenticate a live foreground capability without reporting activity
    /// or extending its lease. Overlay/action reads must not invent input.
    pub(crate) fn validate_live(
        &self,
        uid: u32,
        id: NativeInteractionPublisherId,
    ) -> Result<(), IndexError> {
        {
            let state = self.state.lock().map_err(|error| {
                IndexError::Storage(format!("native activity recorder lock poisoned: {error}"))
            })?;
            let current = state.current.as_ref().ok_or(IndexError::Conflict)?;
            let now = self.clock.now();
            if now < current.last_command_at {
                return Err(IndexError::Storage(
                    "native activity monotonic clock regressed".into(),
                ));
            }
            if current.uid != uid || current.id != id || current.ended || now >= current.valid_until
            {
                return Err(IndexError::Conflict);
            }
            drop(state);
        }
        Ok(())
    }

    fn command(
        &self,
        uid: u32,
        id: NativeInteractionPublisherId,
        sequence: u64,
        command: PublisherCommand,
    ) -> Result<(), IndexError> {
        if sequence == 0 {
            return Err(IndexError::InvalidInput(
                "native activity sequence must be positive".into(),
            ));
        }
        let mut state = self.state.lock().map_err(|error| {
            IndexError::Storage(format!("native activity recorder lock poisoned: {error}"))
        })?;
        let now = self.clock.now();
        let Some(current) = state.current.as_mut() else {
            return Err(IndexError::Conflict);
        };
        if now < current.last_command_at {
            return Err(IndexError::Storage(
                "native activity monotonic clock regressed".into(),
            ));
        }
        if current.uid != uid
            || current.id != id
            || current.ended
            || now >= current.valid_until
            || sequence <= current.last_sequence
        {
            return Err(IndexError::Conflict);
        }
        let valid_until = now.checked_add(current.window).ok_or_else(|| {
            IndexError::InvalidInput("native activity publisher expiry overflow".into())
        })?;
        current.last_sequence = sequence;
        current.last_command_at = now;
        match command {
            PublisherCommand::Activity => {
                current.last_activity_at = Some(now);
                current.valid_until = valid_until;
            }
            PublisherCommand::Renew => current.valid_until = valid_until,
            PublisherCommand::End => {
                current.ended = true;
                current.valid_until = now;
            }
        }
        drop(state);
        Ok(())
    }
}

impl NativeInteractionActivityPort for NativeInteractionRecorder {
    fn observe_activity(&self) -> NativeInteractionActivity {
        let Ok(state) = self.state.lock() else {
            return NativeInteractionActivity::Unknown(UnknownReason::Dropped);
        };
        let now = self.clock.now();
        let Some(current) = state.current else {
            return NativeInteractionActivity::Unknown(UnknownReason::Missing);
        };
        if current.ended {
            return NativeInteractionActivity::Unknown(UnknownReason::Missing);
        }
        if now < current.last_command_at || now >= current.valid_until {
            return NativeInteractionActivity::Unknown(UnknownReason::Dropped);
        }
        let Some(activity) = current.last_activity_at else {
            return NativeInteractionActivity::Unknown(UnknownReason::Missing);
        };
        NativeInteractionActivity::IdleFor {
            idle: now.duration_since(activity),
            valid_for: current.valid_until.duration_since(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{panic::AssertUnwindSafe, sync::MutexGuard};

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) => {
                    if left != right {
                        return Err(format!(
                            "assertion failed: {} == {}; left: {left:?}, right: {right:?}",
                            stringify!($left),
                            stringify!($right)
                        )
                        .into());
                    }
                }
            }
        }};
    }

    struct FakeClock {
        instant: Mutex<Instant>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                instant: Mutex::new(Instant::now()),
            }
        }

        fn shift(&self, duration: Duration, forward: bool) {
            let mut instant = self.instant.lock().expect("fake clock lock");
            *instant = if forward {
                instant.checked_add(duration).expect("fake clock forward")
            } else {
                instant.checked_sub(duration).expect("fake clock backward")
            };
        }
    }

    impl InteractionClock for FakeClock {
        fn now(&self) -> Instant {
            *self.instant.lock().expect("fake clock lock")
        }
    }

    fn fixture() -> (NativeInteractionRecorder, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock::new());
        (NativeInteractionRecorder::with_clock(clock.clone()), clock)
    }

    fn publisher(byte: u8) -> NativeInteractionPublisherId {
        NativeInteractionPublisherId([byte; 32])
    }

    fn poison_state_for_test(recorder: &NativeInteractionRecorder) -> bool {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard: MutexGuard<'_, RecorderState> =
                recorder.state.lock().expect("initial lock");
            panic!("poison native activity recorder");
        }))
        .is_err()
    }

    #[test]
    fn begin_is_unknown_and_only_activity_resets_idle() -> Result<(), Box<dyn std::error::Error>> {
        let (recorder, clock) = fixture();
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Missing)
        );
        recorder.begin(42, publisher(1), Duration::from_secs(2))?;
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Missing)
        );
        clock.shift(Duration::from_millis(500), true);
        recorder.renew(42, publisher(1), 1)?;
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Missing)
        );
        recorder.activity(42, publisher(1), 2)?;
        clock.shift(Duration::from_millis(500), true);
        recorder.renew(42, publisher(1), 3)?;
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(500),
                valid_for: Duration::from_secs(2),
            }
        );
        recorder.activity(42, publisher(1), 4)?;
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::IdleFor {
                idle: Duration::ZERO,
                valid_for: Duration::from_secs(2),
            }
        );
        Ok(())
    }

    #[test]
    fn expired_ended_and_fenced_publishers_cannot_revive() -> Result<(), Box<dyn std::error::Error>>
    {
        let (recorder, clock) = fixture();
        recorder.begin(42, publisher(1), Duration::from_secs(1))?;
        recorder.activity(42, publisher(1), 1)?;
        clock.shift(Duration::from_secs(1), true);
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Dropped)
        );
        check_eq!(
            recorder.activity(42, publisher(1), 2),
            Err(IndexError::Conflict)
        );
        check_eq!(
            recorder.renew(42, publisher(1), 2),
            Err(IndexError::Conflict)
        );
        check_eq!(
            recorder.begin(42, publisher(1), Duration::from_secs(1)),
            Err(IndexError::Conflict)
        );
        recorder.begin(42, publisher(2), Duration::from_secs(1))?;
        check_eq!(
            recorder.activity(42, publisher(1), 3),
            Err(IndexError::Conflict)
        );
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Missing)
        );
        recorder.activity(42, publisher(2), 1)?;
        recorder.end(42, publisher(2), 2)?;
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Missing)
        );
        check_eq!(
            recorder.activity(42, publisher(2), 3),
            Err(IndexError::Conflict)
        );
        recorder.begin(42, publisher(3), Duration::from_secs(1))?;
        check_eq!(
            recorder.renew(42, publisher(2), 4),
            Err(IndexError::Conflict)
        );
        Ok(())
    }

    #[test]
    fn uid_sequence_and_single_publisher_are_fenced_without_state_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (recorder, clock) = fixture();
        recorder.begin(42, publisher(1), Duration::from_secs(2))?;
        check_eq!(
            recorder.begin(42, publisher(2), Duration::from_secs(2)),
            Err(IndexError::Conflict)
        );
        check_eq!(
            recorder.activity(43, publisher(1), 1),
            Err(IndexError::Conflict)
        );
        check_eq!(
            recorder.activity(42, publisher(1), 0),
            Err(IndexError::InvalidInput(
                "native activity sequence must be positive".into()
            ))
        );
        recorder.activity(42, publisher(1), 1)?;
        clock.shift(Duration::from_millis(700), true);
        check_eq!(
            recorder.activity(42, publisher(1), 1),
            Err(IndexError::Conflict)
        );
        check_eq!(
            recorder.renew(42, publisher(1), 1),
            Err(IndexError::Conflict)
        );
        check_eq!(recorder.end(43, publisher(1), 2), Err(IndexError::Conflict));
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(700),
                valid_for: Duration::from_millis(1_300),
            }
        );
        Ok(())
    }

    #[test]
    fn invalid_window_regressed_clock_and_poisoned_lock_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let (recorder, clock) = fixture();
        for window in [
            Duration::ZERO,
            Duration::from_nanos(1),
            Duration::from_secs(61),
        ] {
            check!(matches!(
                recorder.begin(42, publisher(1), window),
                Err(IndexError::InvalidInput(_))
            ));
        }
        check!(matches!(
            recorder.begin(42, publisher(0), Duration::from_secs(1)),
            Err(IndexError::InvalidInput(_))
        ));
        recorder.begin(42, publisher(1), Duration::from_secs(2))?;
        recorder.activity(42, publisher(1), 1)?;
        clock.shift(Duration::from_millis(1), false);
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Dropped)
        );
        check!(matches!(
            recorder.renew(42, publisher(1), 2),
            Err(IndexError::Storage(_))
        ));
        clock.shift(Duration::from_millis(1), true);
        check!(poison_state_for_test(&recorder));
        check_eq!(
            recorder.observe_activity(),
            NativeInteractionActivity::Unknown(UnknownReason::Dropped)
        );
        check!(matches!(
            recorder.activity(42, publisher(1), 2),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }
}
