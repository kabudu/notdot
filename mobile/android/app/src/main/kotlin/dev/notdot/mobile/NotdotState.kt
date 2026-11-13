package dev.notdot.mobile

import android.os.SystemClock
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import uniffi.notdot_mobile_core.Effects
import uniffi.notdot_mobile_core.OrbMode
import uniffi.notdot_mobile_core.OrbQuota
import uniffi.notdot_mobile_core.OrbState
import uniffi.notdot_mobile_core.OrbTask
import uniffi.notdot_mobile_core.PauseReason
import uniffi.notdot_mobile_core.Platform
import uniffi.notdot_mobile_core.SessionEvent
import uniffi.notdot_mobile_core.SessionState
import uniffi.notdot_mobile_core.VoiceSession
import uniffi.notdot_mobile_core.defaultSessionPolicy
import uniffi.notdot_mobile_core.orbDefaultHue

/**
 * Process-wide app state. The session rules live in the Rust core
 * (VoiceSession); Android code feeds it events and applies its effects.
 */
object NotdotState {
    /**
     * Silent test mode: never request or open the microphone, never start
     * the foreground service. Set by instrumented tests and unattended runs.
     */
    @Volatile
    var silent = false

    private var session: VoiceSession? = null
    private val _effects = MutableStateFlow(
        Effects(SessionState.Off, microphone = false, surface = false, endSurface = false, remainingMs = null),
    )
    val effects: StateFlow<Effects> = _effects
    private val _status = MutableStateFlow("Ready")
    val status: StateFlow<String> = _status
    val orb = MutableStateFlow(
        OrbState(
            mode = OrbMode.IDLE, task = OrbTask.NONE, hue = orbDefaultHue(), level = null, reducedMotion = false,
            quota = OrbQuota.NORMAL, taskAvailable = false, microphoneMuted = false, wakeReady = false, offline = true,
        ),
    )

    @Synchronized
    fun session(microphonePermitted: Boolean): VoiceSession =
        session ?: VoiceSession(
            defaultSessionPolicy(Platform.ANDROID).copy(microphonePermitted = microphonePermitted && !silent),
        ).also { session = it }

    @Synchronized
    fun handle(event: SessionEvent, microphonePermitted: Boolean = false): Effects {
        val next = session(microphonePermitted).handle(event, SystemClock.elapsedRealtime().toULong())
        _effects.value = next
        _status.value = describe(next.state)
        orb.value = orb.value.copy(wakeReady = next.microphone)
        return next
    }

    fun setStatus(text: String) {
        _status.value = text
    }

    fun describe(state: SessionState): String = when (state) {
        SessionState.Off -> "Not listening"
        SessionState.Foreground -> "Ready"
        SessionState.HandsFree -> "Listening hands-free"
        is SessionState.Paused -> when (state.reason) {
            PauseReason.INTERRUPTED -> "Paused by a call"
            PauseReason.SILENCED -> "Paused: another app is using the microphone"
            PauseReason.USER_PAUSED -> "Paused"
        }
        is SessionState.Ended -> "Stopped"
    }
}
