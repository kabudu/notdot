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

    /** The paired computer, set by the link screen (null until linked). */
    @Volatile
    var link: uniffi.notdot_mobile_core.PhoneLink? = null

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
    fun handle(event: SessionEvent, microphonePermitted: Boolean? = null): Effects {
        val now = SystemClock.elapsedRealtime().toULong()
        val current = session(microphonePermitted == true)
        if (microphonePermitted != null) {
            current.handle(if (microphonePermitted && !silent) SessionEvent.PermissionGranted else SessionEvent.PermissionLost, now)
        }
        val next = current.handle(event, now)
        _effects.value = next
        if (event != SessionEvent.Tick || orb.value.mode != OrbMode.LISTENING) _status.value = describe(next.state)
        orb.value = orb.value.copy(wakeReady = next.microphone, mode = if (next.microphone) orb.value.mode else OrbMode.IDLE)
        return next
    }

    private val wakeFeedback = java.util.concurrent.atomic.AtomicLong(0)
    fun wakeInput(level: Float, checking: Boolean) {
        if (!effects.value.microphone || orb.value.mode == OrbMode.LISTENING) return
        val hearing = level > 0.006f || checking
        val mode = if (hearing) OrbMode.THINKING else OrbMode.IDLE
        orb.value = orb.value.copy(mode = mode, level = (level * 12f).coerceIn(0f, 1f).toDouble())
        if (hearing) setStatus("Checking wake phrase locally…")
        else setStatus("Listening hands-free · Say Hey Ava")
    }
    fun wakeHeard(linked: Boolean = link != null) {
        handle(SessionEvent.Activity)
        val generation = wakeFeedback.incrementAndGet()
        orb.value = orb.value.copy(mode = OrbMode.LISTENING)
        if (linked) {
            setStatus("Listening…")
            return
        }
        setStatus("Heard you. Link this phone to your computer to talk.")
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            if (wakeFeedback.get() == generation && effects.value.microphone) {
                orb.value = orb.value.copy(mode = OrbMode.IDLE)
                setStatus("Listening hands-free · Say Hey Ava")
            }
        }, 2500)
    }

    /** Show a voice turn's progress (text only; audio is never logged). */
    fun voice(ui: VoiceUi) {
        handle(SessionEvent.Activity)
        val (mode, text) = when (ui) {
            VoiceUi.Listening -> OrbMode.LISTENING to "Listening…"
            VoiceUi.Sending -> OrbMode.THINKING to "Sending to your computer…"
            is VoiceUi.Heard -> OrbMode.THINKING to "“${ui.text}”"
            is VoiceUi.Replying -> OrbMode.SPEAKING to ui.text
            is VoiceUi.Reconnecting -> OrbMode.THINKING to "Reconnecting to your computer…"
            is VoiceUi.Finished -> OrbMode.IDLE to (if (ui.result.cancelled) "Stopped." else "Listening hands-free · Say Hey Ava")
            is VoiceUi.NothingHeard -> OrbMode.IDLE to ui.reason
            is VoiceUi.Failed -> OrbMode.IDLE to ui.reason
        }
        orb.value = orb.value.copy(mode = if (effects.value.microphone) mode else OrbMode.IDLE)
        setStatus(text)
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
