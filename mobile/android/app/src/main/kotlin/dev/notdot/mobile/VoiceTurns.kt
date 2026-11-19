package dev.notdot.mobile

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
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
        if (link.route() == LinkRoute.DISCONNECTED) link.connect(false)
    }
    override suspend fun turn(requestId: String, pcm16: ByteArray, listener: VoiceReplyListener) =
        current().voiceTurn(requestId, pcm16, listener)
    override suspend fun cancel(requestId: String) {
        current().cancelVoice(requestId)
    }
}

/** What the screen shows during a voice turn. */
sealed interface VoiceUi {
    data object Listening : VoiceUi
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
                    send(current.takePcm16())
                }
                CaptureState.NO_SPEECH -> {
                    capture = null
                    ui(VoiceUi.NothingHeard("I didn't hear a request."))
                }
                else -> {}
            }
        }
    }

    private fun send(pcm16: ByteArray) {
        val id = newRequestId()
        requestId = id
        ui(VoiceUi.Sending)
        val listener = object : VoiceReplyListener {
            override fun heard(text: String) { if (requestId == id) ui(VoiceUi.Heard(text)) }
            override fun replyText(text: String) { if (requestId == id) ui(VoiceUi.Replying(text)) }
            override fun replyClip(clip: UInt, mime: String, audio: ByteArray) { if (requestId == id) sink.play(clip, mime, audio) }
            override fun reconnecting(attempt: UInt) { if (requestId == id) ui(VoiceUi.Reconnecting(attempt)) }
        }
        job = scope.launch {
            val outcome = try {
                client.ensureConnected()
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
