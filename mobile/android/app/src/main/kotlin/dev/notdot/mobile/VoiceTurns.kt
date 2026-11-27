package dev.notdot.mobile

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.notdot_mobile_core.CaptureState
import uniffi.notdot_mobile_core.LinkRoute
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.RequestCapture
import uniffi.notdot_mobile_core.VoiceReplyListener
import uniffi.notdot_mobile_core.VoiceTurnResult
import uniffi.notdot_mobile_core.newRequestId

/** The paired computer, as a voice turn needs it (faked in JVM tests). */
interface VoiceTurnClient {
    /** Connect if not connected; throws if the phone isn't linked. */
    suspend fun ensureConnected()
    suspend fun turn(requestId: String, pcm16: ByteArray, listener: VoiceReplyListener): VoiceTurnResult
    suspend fun cancel(requestId: String)
}

/** Plays reply clips in order; `stopAll` drops anything queued or playing. */
interface ReplySink {
    fun play(clip: UInt, mime: String, audio: ByteArray)
    fun stopAll()
}

class PhoneLinkClient(private val link: () -> PhoneLink?) : VoiceTurnClient {
    private fun current() = link() ?: throw MobileException.Offline("Link this phone to your computer first.")
    override suspend fun ensureConnected() {
        val link = current()
        if (link.pairedRecord() == null) throw MobileException.Offline("Link this phone to your computer first.")
        if (link.route() == LinkRoute.DISCONNECTED) {
            val result = try {
                withTimeoutOrNull(8_000) { link.connect(false) }
            } catch (error: MobileException) {
                android.util.Log.i("notdot.turn", "connection failed: ${error.javaClass.simpleName}")
                if (error is MobileException.Unpaired) throw error
                null
            }
            if (result == null || result == LinkRoute.DISCONNECTED) {
                throw MobileException.Offline("Couldn't connect. Check that notdot is running on your computer and you're on the same Wi-Fi.")
            }
        }
    }
    /**
     * Runs the turn and logs its route and timings to `notdot.turn` (never
     * audio or text), so device checks can record which path a turn took.
     */
    override suspend fun turn(requestId: String, pcm16: ByteArray, listener: VoiceReplyListener): VoiceTurnResult {
        val link = current()
        val start = android.os.SystemClock.elapsedRealtime()
        val before = link.route()
        var heardMs = -1L
        var clipMs = -1L
        var clips = 0
        var retries = 0u
        val timed = object : VoiceReplyListener {
            override fun heard(text: String) {
                if (heardMs < 0) heardMs = android.os.SystemClock.elapsedRealtime() - start
                listener.heard(text)
            }
            override fun replyText(text: String) = listener.replyText(text)
            override fun replyClip(clip: UInt, mime: String, audio: ByteArray) {
                if (clipMs < 0) clipMs = android.os.SystemClock.elapsedRealtime() - start
                clips++
                listener.replyClip(clip, mime, audio)
            }
            override fun reconnecting(attempt: UInt) {
                retries = attempt
                android.util.Log.i("notdot.turn", "reconnecting attempt=$attempt after_ms=${android.os.SystemClock.elapsedRealtime() - start}")
                listener.reconnecting(attempt)
            }
        }
        val outcome = runCatching { link.voiceTurn(requestId, pcm16, timed) }
        android.util.Log.i(
            "notdot.turn",
            "turn route_start=$before route_end=${link.route()} request_ms=${pcm16.size / 32} " +
                "heard_ms=$heardMs first_clip_ms=$clipMs clips=$clips reconnects=$retries " +
                "total_ms=${android.os.SystemClock.elapsedRealtime() - start} " +
                "outcome=${outcome.fold({ if (it.cancelled) "cancelled" else "ok" }, { it.javaClass.simpleName })}",
        )
        return outcome.getOrThrow()
    }
    override suspend fun cancel(requestId: String) {
        current().cancelVoice(requestId)
    }
}

/** What the screen shows during a voice turn. */
sealed interface VoiceUi {
    data object Listening : VoiceUi
    data class Connecting(val trailingSilenceMs: UInt) : VoiceUi
    data object Sending : VoiceUi
    data class Heard(val text: String) : VoiceUi
    data class Replying(val text: String) : VoiceUi
    data class Reconnecting(val attempt: UInt) : VoiceUi
    data class Finished(val result: VoiceTurnResult) : VoiceUi
    data class NothingHeard(val reason: String) : VoiceUi
    data class Failed(val reason: String) : VoiceUi
}

/**
 * One voice turn at a time: after the wake phrase, capture the request
 * (endpointed in the Rust core), send it, and play the reply clip by clip.
 * A new wake phrase or Stop cancels the turn in progress. Request audio is
 * kept in memory only and dropped once sent.
 */
class VoiceTurns(
    private val client: VoiceTurnClient,
    private val sink: ReplySink,
    private val scope: CoroutineScope,
    private val ui: (VoiceUi) -> Unit,
) {
    @Volatile private var capture: RequestCapture? = null
    @Volatile private var job: Job? = null
    @Volatile private var requestId: String? = null

    val listening: Boolean get() = capture != null

    /** A request is being sent or its reply is still arriving. */
    val busy: Boolean get() = requestId != null

    /** The wake phrase was heard: start capturing the request. */
    @Synchronized
    fun wake() {
        cancelCurrent()
        capture = RequestCapture()
        ui(VoiceUi.Listening)
    }

    /** Microphone audio (16 kHz mono PCM16 LE) after the wake phrase. */
    fun feed(pcm16: ByteArray) {
        val current = capture ?: return
        val state = try { current.pushPcm16(pcm16) } catch (e: MobileException) { return }
        synchronized(this) {
            if (capture !== current) return
            when (state) {
                CaptureState.DONE, CaptureState.TOO_LONG -> {
                    capture = null
                    val trailingSilenceMs = current.trailingSilenceMs()
                    send(current.takePcm16(), trailingSilenceMs)
                }
                CaptureState.NO_SPEECH -> {
                    capture = null
                    ui(VoiceUi.NothingHeard("I didn't hear a request. Tap and try speaking again."))
                }
                else -> {}
            }
        }
    }

    private fun send(pcm16: ByteArray, trailingSilenceMs: UInt) {
        val id = newRequestId()
        requestId = id
        ui(VoiceUi.Connecting(trailingSilenceMs))
        val listener = object : VoiceReplyListener {
            override fun heard(text: String) { if (requestId == id) ui(VoiceUi.Heard(text)) }
            override fun replyText(text: String) { if (requestId == id) ui(VoiceUi.Replying(text)) }
            override fun replyClip(clip: UInt, mime: String, audio: ByteArray) { if (requestId == id) sink.play(clip, mime, audio) }
            override fun reconnecting(attempt: UInt) { if (requestId == id) ui(VoiceUi.Reconnecting(attempt)) }
        }
        job = scope.launch {
            val outcome = try {
                client.ensureConnected()
                if (requestId != id) return@launch
                ui(VoiceUi.Sending)
                VoiceUi.Finished(client.turn(id, pcm16, listener))
            } catch (e: MobileException) {
                VoiceUi.Failed(e.plain())
            }
            if (requestId == id) {
                requestId = null
                ui(outcome)
            }
        }
    }

    @Synchronized
    fun microphoneFailed() {
        if (capture == null) return
        cancelCurrent()
        ui(VoiceUi.Failed("The microphone stopped before I could send your request. Tap to try again."))
    }

    /** Stop: drop the capture, stop playback, and cancel the turn there too. */
    @Synchronized
    fun stop() = cancelCurrent()

    private fun cancelCurrent() {
        capture = null
        sink.stopAll()
        val id = requestId ?: return
        requestId = null
        scope.launch { runCatching { client.cancel(id) } }
    }
}

/** The core's plain-language detail (every variant carries one). */
fun MobileException.plain(): String =
    message?.removePrefix("detail=")?.takeIf { it.isNotBlank() } ?: "Couldn't reach your computer."
