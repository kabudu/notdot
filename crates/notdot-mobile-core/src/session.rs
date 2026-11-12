//! The hands-free voice session lifecycle, shared by iOS and Android.
//!
//! A pure state machine: the native shell feeds it events (app visibility,
//! user actions, wake hits, interruptions, ticks) and applies the returned
//! [`Effects`] (microphone on/off, session surface shown/ended). Keeping the
//! rules here means both OSes stop, pause and time out identically.
//!
//! Two lifetimes are kept apart, as the platforms document them:
//! - The microphone/audio session. iOS: recording continues in the
//!   background with the `audio` background mode; no time limit is
//!   documented. Android: a `microphone` foreground service.
//! - The session surface. iOS: a Live Activity, which the system ends after
//!   at most eight hours; a session started by `AudioRecordingIntent` must
//!   keep its Live Activity active while recording. Android: the foreground
//!   service notification, with no system time limit.
//!
//! notdot ties a hands-free session to its visible surface on both OSes, so
//! when the surface ends the session ends. On iOS that makes eight hours the
//! longest session; that limit comes from the Live Activity, not from the
//! audio session.
use std::time::Duration;

/// Default idle timeout (owner decision): ten minutes without a wake or turn.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const MIN_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(8 * 60 * 60);
/// Documented Live Activity limit (ActivityKit: "up to eight hours").
pub const IOS_SURFACE_LIMIT: Duration = Duration::from_secs(8 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Platform {
    Ios,
    Android,
}

/// How a hands-free session was started; all are user actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StartSource {
    /// Button in the visible app.
    App,
    /// iOS App Intent (Action button, Control, Siri phrase); Android
    /// widget, tile-launched activity or notification action.
    SystemAction,
    /// Headset/media button while the app is visible.
    Headset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum EndReason {
    UserStopped,
    /// "That's all".
    SpokenStop,
    IdleTimeout,
    /// The Live Activity or notification went away (system limit, user
    /// dismissed it, or Live Activities are disabled).
    SurfaceEnded,
    PermissionLost,
    /// The OS stopped the app's capture (force-stop, service killed).
    SystemStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PauseReason {
    /// A call or another higher-priority audio session.
    Interrupted,
    /// Android delivers silence to this app (`isClientSilenced`).
    Silenced,
    /// The user said "Pause listening".
    UserPaused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SessionState {
    /// Nothing may capture: app not visible and no session.
    Off,
    /// App visible; wake listening while it stays visible.
    Foreground,
    /// User-started session; listening continues when hidden or locked.
    HandsFree,
    Paused {
        reason: PauseReason,
    },
    Ended {
        reason: EndReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SessionEvent {
    AppVisible,
    AppHidden,
    Start {
        source: StartSource,
    },
    Stop,
    SpokenStop,
    /// A wake word or completed turn: activity that resets the idle timer.
    Activity,
    PauseListening,
    ResumeListening,
    InterruptionBegan,
    InterruptionEnded {
        should_resume: bool,
    },
    Silenced {
        silenced: bool,
    },
    SurfaceEnded,
    PermissionLost,
    SystemStopped,
    /// Periodic tick so timeouts apply without other events.
    Tick,
}

/// What the shell must make true after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Effects {
    pub state: SessionState,
    /// Capture and wake detection should run.
    pub microphone: bool,
    /// The Live Activity (iOS) or foreground-service notification (Android)
    /// should be showing.
    pub surface: bool,
    /// The surface was just ended by notdot and must be dismissed now.
    pub end_surface: bool,
    /// Milliseconds until the session would time out (for the surface).
    pub remaining_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SessionPolicy {
    pub platform: Platform,
    pub idle_timeout_ms: u64,
    /// Wake listening while the app is visible.
    pub wake_enabled: bool,
    pub microphone_permitted: bool,
}

/// Clamp a user-chosen idle timeout to the supported range.
#[uniffi::export]
pub fn clamp_idle_timeout_ms(requested: u64) -> u64 {
    requested.clamp(
        MIN_IDLE_TIMEOUT.as_millis() as u64,
        MAX_IDLE_TIMEOUT.as_millis() as u64,
    )
}

#[uniffi::export]
pub fn default_session_policy(platform: Platform) -> SessionPolicy {
    SessionPolicy {
        platform,
        idle_timeout_ms: DEFAULT_IDLE_TIMEOUT.as_millis() as u64,
        wake_enabled: true,
        microphone_permitted: false,
    }
}

#[derive(Debug)]
pub struct Session {
    policy: SessionPolicy,
    state: SessionState,
    visible: bool,
    started_ms: u64,
    last_activity_ms: u64,
    /// Pause reasons can overlap (paused by the user, then a call).
    user_paused: bool,
    interrupted: bool,
    silenced: bool,
}

impl Session {
    pub fn new(policy: SessionPolicy) -> Self {
        let policy = SessionPolicy {
            idle_timeout_ms: clamp_idle_timeout_ms(policy.idle_timeout_ms),
            ..policy
        };
        Self {
            policy,
            state: SessionState::Off,
            visible: false,
            started_ms: 0,
            last_activity_ms: 0,
            user_paused: false,
            interrupted: false,
            silenced: false,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    fn in_session(&self) -> bool {
        matches!(
            self.state,
            SessionState::HandsFree | SessionState::Paused { .. }
        )
    }

    fn idle_state(&self) -> SessionState {
        if self.visible {
            SessionState::Foreground
        } else {
            SessionState::Off
        }
    }

    fn deadline_ms(&self) -> u64 {
        let idle = self
            .last_activity_ms
            .saturating_add(self.policy.idle_timeout_ms);
        match self.policy.platform {
            Platform::Ios => idle.min(
                self.started_ms
                    .saturating_add(IOS_SURFACE_LIMIT.as_millis() as u64),
            ),
            Platform::Android => idle,
        }
    }

    fn end(&mut self, reason: EndReason) {
        self.state = SessionState::Ended { reason };
        self.user_paused = false;
        self.interrupted = false;
        self.silenced = false;
    }

    fn settle_pause(&mut self) {
        if !self.in_session() {
            return;
        }
        self.state = if self.interrupted {
            SessionState::Paused {
                reason: PauseReason::Interrupted,
            }
        } else if self.silenced {
            SessionState::Paused {
                reason: PauseReason::Silenced,
            }
        } else if self.user_paused {
            SessionState::Paused {
                reason: PauseReason::UserPaused,
            }
        } else {
            SessionState::HandsFree
        };
    }

    /// Apply one event at `now_ms` (monotonic milliseconds).
    pub fn handle(&mut self, event: SessionEvent, now_ms: u64) -> Effects {
        let was_in_session = self.in_session();
        // An ended session is reported once, then rests.
        if matches!(self.state, SessionState::Ended { .. }) {
            self.state = self.idle_state();
        }
        match event {
            SessionEvent::AppVisible => {
                self.visible = true;
                if !self.in_session() {
                    self.state = SessionState::Foreground;
                }
            }
            SessionEvent::AppHidden => {
                self.visible = false;
                if !self.in_session() {
                    self.state = SessionState::Off;
                }
            }
            SessionEvent::Start { source } => {
                // Android cannot start microphone capture from the
                // background except via exempt system actions; iOS needs
                // the app visible or an App Intent. The shell only sends
                // Start for those cases; refuse anything else here too.
                let allowed = self.visible || source == StartSource::SystemAction;
                if allowed && self.policy.microphone_permitted && !self.in_session() {
                    self.state = SessionState::HandsFree;
                    self.started_ms = now_ms;
                    self.last_activity_ms = now_ms;
                }
            }
            SessionEvent::Stop if self.in_session() => self.end(EndReason::UserStopped),
            SessionEvent::SpokenStop if self.in_session() => self.end(EndReason::SpokenStop),
            SessionEvent::Activity if self.state == SessionState::HandsFree => {
                self.last_activity_ms = now_ms;
            }
            SessionEvent::PauseListening => {
                self.user_paused = true;
                self.settle_pause();
            }
            SessionEvent::ResumeListening => {
                self.user_paused = false;
                self.last_activity_ms = now_ms;
                self.settle_pause();
            }
            SessionEvent::InterruptionBegan => {
                self.interrupted = true;
                self.settle_pause();
            }
            SessionEvent::InterruptionEnded { should_resume } => {
                self.interrupted = false;
                if should_resume {
                    self.last_activity_ms = now_ms;
                } else if self.in_session() {
                    // The system advises against resuming: stay paused
                    // until the user resumes.
                    self.user_paused = true;
                }
                self.settle_pause();
            }
            SessionEvent::Silenced { silenced } => {
                self.silenced = silenced;
                self.settle_pause();
            }
            SessionEvent::SurfaceEnded if self.in_session() => self.end(EndReason::SurfaceEnded),
            SessionEvent::PermissionLost => {
                self.policy.microphone_permitted = false;
                if self.in_session() {
                    self.end(EndReason::PermissionLost);
                }
            }
            SessionEvent::SystemStopped if self.in_session() => self.end(EndReason::SystemStopped),
            _ => {}
        }
        // Timeouts apply on every event, including Tick. Time paused by a
        // call or by the user does not count as idle on resume (resume
        // resets activity), but the iOS surface limit always applies.
        if self.in_session() && now_ms >= self.deadline_ms() {
            let surface_limit = self.policy.platform == Platform::Ios
                && now_ms
                    >= self
                        .started_ms
                        .saturating_add(IOS_SURFACE_LIMIT.as_millis() as u64);
            if self.state == SessionState::HandsFree || surface_limit {
                self.end(if surface_limit {
                    EndReason::SurfaceEnded
                } else {
                    EndReason::IdleTimeout
                });
            }
        }
        let in_session = self.in_session();
        let microphone = match self.state {
            SessionState::HandsFree => true,
            SessionState::Foreground => {
                self.policy.wake_enabled && self.policy.microphone_permitted
            }
            _ => false,
        };
        Effects {
            state: self.state,
            microphone,
            surface: in_session,
            end_surface: was_in_session && !in_session,
            remaining_ms: in_session.then(|| self.deadline_ms().saturating_sub(now_ms)),
        }
    }
}

/// Thread-safe wrapper for the native shells.
#[derive(uniffi::Object)]
pub struct VoiceSession(std::sync::Mutex<Session>);

#[uniffi::export]
impl VoiceSession {
    #[uniffi::constructor]
    pub fn new(policy: SessionPolicy) -> Self {
        Self(std::sync::Mutex::new(Session::new(policy)))
    }
    pub fn handle(&self, event: SessionEvent, now_ms: u64) -> Effects {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(event, now_ms)
    }
    pub fn state(&self) -> SessionState {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn session(platform: Platform) -> Session {
        Session::new(SessionPolicy {
            microphone_permitted: true,
            ..default_session_policy(platform)
        })
    }

    #[test]
    fn foreground_wake_only_while_visible_and_never_hidden_without_a_session() {
        let mut s = session(Platform::Android);
        let e = s.handle(SessionEvent::AppVisible, 0);
        assert_eq!(
            (e.state, e.microphone, e.surface),
            (SessionState::Foreground, true, false)
        );
        let e = s.handle(SessionEvent::AppHidden, 1);
        assert_eq!((e.state, e.microphone), (SessionState::Off, false));
        // Starting from the background is refused unless a system action.
        let e = s.handle(
            SessionEvent::Start {
                source: StartSource::App,
            },
            2,
        );
        assert_eq!(e.state, SessionState::Off);
        let e = s.handle(
            SessionEvent::Start {
                source: StartSource::SystemAction,
            },
            3,
        );
        assert_eq!(
            (e.state, e.microphone, e.surface),
            (SessionState::HandsFree, true, true)
        );
    }

    #[test]
    fn hands_free_survives_hiding_and_ends_after_the_idle_timeout() {
        let mut s = session(Platform::Ios);
        s.handle(SessionEvent::AppVisible, 0);
        s.handle(
            SessionEvent::Start {
                source: StartSource::App,
            },
            0,
        );
        let e = s.handle(SessionEvent::AppHidden, 1_000);
        assert_eq!((e.state, e.microphone), (SessionState::HandsFree, true));
        assert_eq!(e.remaining_ms, Some(10 * MIN - 1_000));
        // Activity pushes the deadline out.
        s.handle(SessionEvent::Activity, 9 * MIN);
        assert_eq!(
            s.handle(SessionEvent::Tick, 10 * MIN).state,
            SessionState::HandsFree
        );
        let e = s.handle(SessionEvent::Tick, 19 * MIN);
        assert_eq!(
            e.state,
            SessionState::Ended {
                reason: EndReason::IdleTimeout
            }
        );
        assert!(e.end_surface && !e.microphone && !e.surface);
        // Hidden afterwards: capture is off.
        assert_eq!(
            s.handle(SessionEvent::Tick, 20 * MIN).state,
            SessionState::Off
        );
    }

    #[test]
    fn ios_sessions_end_with_their_live_activity_at_eight_hours_android_does_not() {
        for (platform, expect_end) in [(Platform::Ios, true), (Platform::Android, false)] {
            let mut s = Session::new(SessionPolicy {
                idle_timeout_ms: u64::MAX,
                microphone_permitted: true,
                ..default_session_policy(platform)
            });
            s.handle(SessionEvent::AppVisible, 0);
            s.handle(
                SessionEvent::Start {
                    source: StartSource::App,
                },
                0,
            );
            let eight_hours = IOS_SURFACE_LIMIT.as_millis() as u64;
            // Keep it busy, so only the surface limit can end it.
            let mut now = 0;
            while now + 30 * MIN < eight_hours {
                now += 30 * MIN;
                s.handle(SessionEvent::Activity, now);
            }
            let state = s.handle(SessionEvent::Tick, eight_hours).state;
            assert_eq!(
                matches!(
                    state,
                    SessionState::Ended {
                        reason: EndReason::SurfaceEnded
                    }
                ),
                expect_end,
                "{platform:?} {state:?}"
            );
        }
    }

    #[test]
    fn interruptions_pause_and_resume_only_when_the_system_says_so() {
        let mut s = session(Platform::Ios);
        s.handle(SessionEvent::AppVisible, 0);
        s.handle(
            SessionEvent::Start {
                source: StartSource::App,
            },
            0,
        );
        let e = s.handle(SessionEvent::InterruptionBegan, 1);
        assert_eq!(
            (e.state, e.microphone, e.surface),
            (
                SessionState::Paused {
                    reason: PauseReason::Interrupted
                },
                false,
                true
            )
        );
        // A long call does not time the session out while paused.
        let e = s.handle(SessionEvent::Tick, 30 * MIN);
        assert!(matches!(e.state, SessionState::Paused { .. }));
        let e = s.handle(
            SessionEvent::InterruptionEnded {
                should_resume: true,
            },
            31 * MIN,
        );
        assert_eq!((e.state, e.microphone), (SessionState::HandsFree, true));
        s.handle(SessionEvent::InterruptionBegan, 32 * MIN);
        let e = s.handle(
            SessionEvent::InterruptionEnded {
                should_resume: false,
            },
            33 * MIN,
        );
        assert_eq!(
            e.state,
            SessionState::Paused {
                reason: PauseReason::UserPaused
            }
        );
        let e = s.handle(SessionEvent::ResumeListening, 34 * MIN);
        assert_eq!(e.state, SessionState::HandsFree);
    }

    #[test]
    fn overlapping_pauses_resolve_in_priority_order() {
        let mut s = session(Platform::Android);
        s.handle(SessionEvent::AppVisible, 0);
        s.handle(
            SessionEvent::Start {
                source: StartSource::Headset,
            },
            0,
        );
        s.handle(SessionEvent::PauseListening, 1);
        s.handle(SessionEvent::Silenced { silenced: true }, 2);
        let e = s.handle(SessionEvent::InterruptionBegan, 3);
        assert_eq!(
            e.state,
            SessionState::Paused {
                reason: PauseReason::Interrupted
            }
        );
        s.handle(
            SessionEvent::InterruptionEnded {
                should_resume: true,
            },
            4,
        );
        assert_eq!(
            s.state(),
            SessionState::Paused {
                reason: PauseReason::Silenced
            }
        );
        s.handle(SessionEvent::Silenced { silenced: false }, 5);
        assert_eq!(
            s.state(),
            SessionState::Paused {
                reason: PauseReason::UserPaused
            }
        );
    }

    #[test]
    fn every_stop_path_ends_once_and_releases_the_microphone() {
        for (event, reason) in [
            (SessionEvent::Stop, EndReason::UserStopped),
            (SessionEvent::SpokenStop, EndReason::SpokenStop),
            (SessionEvent::SurfaceEnded, EndReason::SurfaceEnded),
            (SessionEvent::PermissionLost, EndReason::PermissionLost),
            (SessionEvent::SystemStopped, EndReason::SystemStopped),
        ] {
            let mut s = session(Platform::Android);
            s.handle(SessionEvent::AppVisible, 0);
            s.handle(
                SessionEvent::Start {
                    source: StartSource::App,
                },
                0,
            );
            s.handle(SessionEvent::AppHidden, 1);
            let e = s.handle(event, 2);
            assert_eq!(e.state, SessionState::Ended { reason });
            assert!(e.end_surface && !e.microphone);
            let again = s.handle(event, 3);
            assert!(!again.end_surface, "{event:?} ended twice");
            assert!(!again.microphone);
        }
    }

    #[test]
    fn no_session_without_microphone_permission_and_timeouts_are_clamped() {
        let mut s = Session::new(default_session_policy(Platform::Ios));
        s.handle(SessionEvent::AppVisible, 0);
        let e = s.handle(
            SessionEvent::Start {
                source: StartSource::App,
            },
            0,
        );
        assert_eq!((e.state, e.microphone), (SessionState::Foreground, false));
        assert_eq!(clamp_idle_timeout_ms(0), MIN);
        assert_eq!(clamp_idle_timeout_ms(u64::MAX), 8 * 60 * MIN);
        assert_eq!(
            default_session_policy(Platform::Ios).idle_timeout_ms,
            10 * MIN
        );
    }
}
