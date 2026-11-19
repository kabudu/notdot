package dev.notdot.mobile

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/** 16 kHz mono PCM16 microphone audio for one request (faked in JVM tests). */
interface PcmSource {
    /** Fills [buffer]; returns the bytes read, or <= 0 if capture failed. */
    fun read(buffer: ByteArray): Int
    fun close()
}

enum class TalkState { IDLE, LISTENING, REPLYING }

/**
 * Tap to talk, the default way to speak to the paired computer: no wake
 * word. A tap opens the microphone (only while the app is in use), captures
 * one request until the core's endpointer says it ended, sends it and plays
 * the reply. A tap while listening or replying stops everything, here and
 * on the computer. The microphone closes as soon as the request ends.
 */
class TapToTalk(
    private val turns: VoiceTurns,
    private val open: () -> PcmSource?,
    private val playing: () -> Boolean,
    private val onState: (TalkState) -> Unit,
    private val run: (Runnable) -> Unit = { Thread(it, "notdot-talk").start() },
) {
    private val lock = Any()
    private var source: PcmSource? = null

    val state: TalkState
        get() = synchronized(lock) {
            when {
                source != null -> TalkState.LISTENING
                turns.busy || playing() -> TalkState.REPLYING
                else -> TalkState.IDLE
            }
        }

    /** The talk button. False only if the microphone could not open. */
    fun tap(): Boolean {
        synchronized(lock) {
            if (state != TalkState.IDLE) {
                stopLocked()
            } else {
                val src = open() ?: return false
                source = src
                turns.wake()
                run(Runnable { pump(src) })
            }
        }
        changed()
        return true
    }

    /** The app was hidden: an unfinished request is dropped (a reply keeps playing). */
    fun hidden() {
        synchronized(lock) { if (source != null) stopLocked() }
        changed()
    }

    /** Re-publish the state (turn and playback events call this). */
    fun changed() = onState(state)

    private fun stopLocked() {
        source = null // the capture thread closes it
        turns.stop()
    }

    private fun pump(src: PcmSource) {
        val packet = ByteArray(PACKET_BYTES)
        var failed = false
        try {
            while (synchronized(lock) { source === src } && turns.listening) {
                val n = src.read(packet)
                if (n <= 0) {
                    failed = true
                    break
                }
                turns.feed(packet.copyOf(n))
            }
        } finally {
            val mine = synchronized(lock) { (source === src).also { if (it) source = null } }
            src.close()
            if (mine && failed && turns.listening) turns.stop()
            changed()
        }
    }

    companion object {
        const val PACKET_BYTES = 640 // 20 ms
    }
}

/** The app's one tap-to-talk path (its own turn controller and reply player). */
object TalkPath {
    private val _state = MutableStateFlow(TalkState.IDLE)
    val state: StateFlow<TalkState> = _state
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var tap: TapToTalk? = null

    @Synchronized
    fun get(): TapToTalk = tap ?: run {
        var self: TapToTalk? = null
        val player = ReplyPlayer { self?.changed() }
        val turns = VoiceTurns(PhoneLinkClient { NotdotState.link }, player, scope) { ui ->
            NotdotState.voice(ui)
            self?.changed()
        }
        TapToTalk(turns, ::microphone, { player.playing }, { _state.value = it }).also {
            self = it
            tap = it
        }
    }

    @SuppressLint("MissingPermission") // the caller checks RECORD_AUDIO first
    private fun microphone(): PcmSource? {
        val rate = 16_000
        val min = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (min <= 0) return null
        val record = try {
            AudioRecord(
                MediaRecorder.AudioSource.VOICE_RECOGNITION, rate, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(min, rate * 2 / 2),
            )
        } catch (_: RuntimeException) {
            return null
        }
        return try {
            check(record.state == AudioRecord.STATE_INITIALIZED)
            record.startRecording()
            check(record.recordingState == AudioRecord.RECORDSTATE_RECORDING)
            object : PcmSource {
                override fun read(buffer: ByteArray) =
                    try { record.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING) } catch (_: IllegalStateException) { -1 }
                override fun close() {
                    runCatching { record.stop() }
                    record.release()
                }
            }
        } catch (_: RuntimeException) {
            record.release()
            null
        }
    }
}
