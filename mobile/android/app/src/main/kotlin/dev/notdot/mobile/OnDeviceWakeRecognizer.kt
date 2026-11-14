package dev.notdot.mobile

import android.content.Context
import android.content.Intent
import android.media.AudioFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.speech.RecognitionListener
import android.speech.RecognitionSupport
import android.speech.RecognitionSupportCallback
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import androidx.annotation.RequiresApi
import java.io.Closeable
import java.io.IOException
import java.io.OutputStream
import java.util.Locale
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * Wake detection with the phone's own on-device speech recogniser
 * (Android's `SpeechRecognizer.createOnDeviceSpeechRecognizer`), fed audio
 * the app has already captured. No model is bundled, and there is no cloud
 * fallback: `createSpeechRecognizer` is never used, and the recogniser is
 * only started when the requested language is installed on the device.
 *
 * Contract:
 * - [start], [finishAudio] and [stop] are called on the main thread; so are
 *   all [Listener] callbacks. [feedPcm16] may be called from any thread and
 *   never blocks: it queues at most [PcmPipeWriter.DEFAULT_MAX_BYTES] of audio
 *   and drops the oldest audio beyond that (counted in [stats]).
 * - Audio is 16-bit little-endian PCM, 16 kHz, mono; the caller converts.
 * - A wake is reported only for a final (segment or end) result whose best
 *   hypothesis starts with the whole wake phrase ([WakePhraseMatcher]).
 *   Partial results never wake.
 * - Nothing is retried automatically. When a session ends or fails, the
 *   listener is told why and the caller decides whether to [start] again.
 * - After [stop], or a new [start] (for example a name change), no callback
 *   from the earlier session is delivered.
 * - Speech and transcripts are never stored or logged; [stats] holds counts.
 *
 * Needs Android 13 (API 33) for an app-supplied audio source, segmented
 * sessions and biasing strings; below that, [start] reports
 * [WakeRecognizerError.Unsupported]. The foreground microphone service and
 * RECORD_AUDIO permission remain the caller's responsibility.
 */
class OnDeviceWakeRecognizer(
    private val context: Context,
    private val listener: Listener,
    private val language: String = "en-US",
) {
    interface Listener {
        fun onStatus(status: WakeRecognizerStatus)
        fun onWake()
        fun onError(error: WakeRecognizerError)
    }

    private val main = Handler(Looper.getMainLooper())
    private var generation = 0L
    private var recognizer: SpeechRecognizer? = null
    private var source: ParcelFileDescriptor? = null
    @Volatile private var writer: PcmPipeWriter? = null
    private var matcher: WakePhraseMatcher? = null
    private var counts = WakeRecognizerStats()

    /** Recognition counts for this recogniser (never audio or text). */
    fun stats(): WakeRecognizerStats = counts.copy(droppedBytes = counts.droppedBytes + (writer?.droppedBytes ?: 0))

    /**
     * Start listening for "Hey [name]" (or [name] alone). Stops any current
     * session first. Reports progress through [Listener.onStatus], and any
     * reason it cannot listen through [Listener.onError].
     */
    fun start(name: String, nameOnly: Boolean = false) {
        requireMain()
        stop()
        val session = generation
        val phrase = WakePhraseMatcher.phrase(name, nameOnly)
        if (phrase == null) {
            listener.onError(WakeRecognizerError.InvalidName)
            return
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            listener.onError(WakeRecognizerError.Unsupported("Android 13 or later is needed to recognise app-supplied audio on the device"))
            return
        }
        startOnDevice(session, phrase)
    }

    /**
     * Queue captured audio (16-bit little-endian PCM, 16 kHz, mono). Any
     * thread; never blocks. False if nothing is listening, the chunk is not
     * whole samples, or the queue was full (oldest audio dropped).
     */
    fun feedPcm16(bytes: ByteArray): Boolean = writer?.offer(bytes) ?: false

    /**
     * End the audio stream: the recogniser finishes the last segment, then
     * reports the session's end. For feeding a recording through the same
     * path as live audio.
     */
    fun finishAudio() {
        requireMain()
        writer?.finish()
    }

    /** Stop listening and release the recogniser and audio pipe. */
    fun stop() {
        requireMain()
        generation++
        release()
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private fun startOnDevice(session: Long, phrase: String) {
        if (!SpeechRecognizer.isOnDeviceRecognitionAvailable(context)) {
            listener.onError(WakeRecognizerError.Unsupported("This device has no on-device speech recogniser"))
            return
        }
        val created = try {
            SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        } catch (_: UnsupportedOperationException) {
            listener.onError(WakeRecognizerError.Unsupported("This device has no on-device speech recogniser"))
            return
        }
        recognizer = created
        matcher = WakePhraseMatcher(phrase)
        counts = WakeRecognizerStats()
        created.setRecognitionListener(Callbacks(session))
        listener.onStatus(WakeRecognizerStatus.CheckingLanguage)
        created.checkRecognitionSupport(
            baseIntent(phrase),
            context.mainExecutor,
            object : RecognitionSupportCallback {
                override fun onSupportResult(support: RecognitionSupport) {
                    if (session != generation) return
                    when {
                        support.installedOnDeviceLanguages.any { it.matches(language) } -> listen(session, phrase, verified = true)
                        else -> fail(
                            WakeRecognizerError.LanguageNotInstalled(
                                language,
                                downloadable = support.supportedOnDeviceLanguages.any { it.matches(language) },
                                pending = support.pendingOnDeviceLanguages.any { it.matches(language) },
                            ),
                        )
                    }
                }

                override fun onError(error: Int) {
                    if (session != generation) return
                    // Some services cannot answer; the start itself then
                    // reports a missing language (ERROR_LANGUAGE_*).
                    if (error == SpeechRecognizer.ERROR_CANNOT_CHECK_SUPPORT) {
                        listen(session, phrase, verified = false)
                    } else {
                        fail(WakeRecognizerError.Recognizer(error))
                    }
                }
            },
        )
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private fun listen(session: Long, phrase: String, verified: Boolean) {
        val pipe = try {
            ParcelFileDescriptor.createPipe()
        } catch (_: IOException) {
            fail(WakeRecognizerError.AudioPipe)
            return
        }
        source = pipe[0]
        writer = PcmPipeWriter(ParcelFileDescriptor.AutoCloseOutputStream(pipe[1])) {
            main.post { if (session == generation) fail(WakeRecognizerError.AudioPipe) }
        }
        val intent = baseIntent(phrase).apply {
            putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE, pipe[0])
            putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
            putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_SAMPLING_RATE, SAMPLE_RATE)
            putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_CHANNEL_COUNT, 1)
            // One session until the audio is closed, reported per segment.
            putExtra(RecognizerIntent.EXTRA_SEGMENTED_SESSION, RecognizerIntent.EXTRA_AUDIO_SOURCE)
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        }
        recognizer?.startListening(intent)
        listener.onStatus(WakeRecognizerStatus.Listening(languageVerified = verified))
    }

    private fun baseIntent(phrase: String) = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
        putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
        putExtra(RecognizerIntent.EXTRA_LANGUAGE, language)
        putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            putStringArrayListExtra(RecognizerIntent.EXTRA_BIASING_STRINGS, arrayListOf(phrase))
            putExtra(RecognizerIntent.EXTRA_ENABLE_BIASING_DEVICE_CONTEXT, false)
        }
    }

    private fun String.matches(tag: String) = equals(tag, ignoreCase = true) ||
        replace('_', '-').equals(tag.replace('_', '-'), ignoreCase = true)

    /** End this session after a failure; no retry. */
    private fun fail(error: WakeRecognizerError) {
        generation++
        release()
        listener.onError(error)
    }

    private fun release() {
        writer?.let { counts = counts.copy(droppedBytes = counts.droppedBytes + it.droppedBytes) }
        writer?.close()
        writer = null
        recognizer?.let {
            it.cancel()
            it.destroy()
        }
        recognizer = null
        try {
            source?.close()
        } catch (_: IOException) {
        }
        source = null
        matcher = null
    }

    private fun requireMain() = check(Looper.myLooper() == Looper.getMainLooper()) {
        "OnDeviceWakeRecognizer must be used on the main thread"
    }

    /** Callbacks for one session; anything from an older session is dropped. */
    private inner class Callbacks(private val session: Long) : RecognitionListener {
        private val current get() = session == generation

        private fun results(bundle: Bundle?) {
            val best = bundle?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()
            counts = counts.copy(segments = counts.segments + 1)
            if (best != null && matcher?.matches(best) == true) {
                counts = counts.copy(wakes = counts.wakes + 1)
                listener.onWake()
            }
        }

        override fun onSegmentResults(segmentResults: Bundle) {
            if (current) results(segmentResults)
        }

        override fun onResults(results: Bundle?) {
            if (!current) return
            results(results)
            ended("results")
        }

        override fun onEndOfSegmentedSession() {
            if (current) ended("audio ended")
        }

        override fun onPartialResults(partialResults: Bundle?) {
            // Never a wake: a partial name may still change.
            if (current) counts = counts.copy(partials = counts.partials + 1)
        }

        override fun onError(error: Int) {
            if (current) fail(WakeRecognizerError.Recognizer(error))
        }

        private fun ended(reason: String) {
            generation++
            release()
            listener.onStatus(WakeRecognizerStatus.Ended(reason))
        }

        override fun onReadyForSpeech(params: Bundle?) = Unit
        override fun onBeginningOfSpeech() = Unit
        override fun onRmsChanged(rmsdB: Float) = Unit
        override fun onBufferReceived(buffer: ByteArray?) = Unit
        override fun onEndOfSpeech() = Unit
        override fun onEvent(eventType: Int, params: Bundle?) = Unit
    }

    companion object {
        const val SAMPLE_RATE = 16_000
    }
}

sealed interface WakeRecognizerStatus {
    data object CheckingLanguage : WakeRecognizerStatus
    /** `languageVerified` is false when the service could not report support. */
    data class Listening(val languageVerified: Boolean) : WakeRecognizerStatus
    /** The session ended (audio finished, or the service ended it). */
    data class Ended(val reason: String) : WakeRecognizerStatus
}

sealed interface WakeRecognizerError {
    data object InvalidName : WakeRecognizerError
    data class Unsupported(val reason: String) : WakeRecognizerError
    /** Not installed on the device. Nothing is downloaded automatically. */
    data class LanguageNotInstalled(val language: String, val downloadable: Boolean, val pending: Boolean) : WakeRecognizerError
    /** A `SpeechRecognizer.ERROR_*` code. */
    data class Recognizer(val code: Int) : WakeRecognizerError {
        val name: String get() = errorName(code)
    }
    data object AudioPipe : WakeRecognizerError
}

/** Counts only; no audio or text. */
data class WakeRecognizerStats(
    val segments: Long = 0,
    val partials: Long = 0,
    val wakes: Long = 0,
    val droppedBytes: Long = 0,
)

fun errorName(code: Int): String = when (code) {
    1 -> "network timeout"
    2 -> "network"
    3 -> "audio"
    4 -> "server"
    5 -> "client"
    6 -> "speech timeout"
    7 -> "no match"
    8 -> "recogniser busy"
    9 -> "insufficient permissions"
    10 -> "too many requests"
    11 -> "server disconnected"
    12 -> "language not supported"
    13 -> "language unavailable"
    14 -> "cannot check support"
    15 -> "cannot listen to download events"
    else -> "error $code"
}

/**
 * Whether a final transcript is the wake phrase: its words must start with
 * the phrase's words exactly (case and punctuation ignored), so "Hey Ava,
 * what's on today" wakes but "Hey Avalon", "Hey Eva" and "Ava says hey" do
 * not. No spelling aliases.
 */
class WakePhraseMatcher(phrase: String) {
    private val words = words(phrase)

    fun matches(transcript: String): Boolean {
        val heard = words(transcript)
        return words.isNotEmpty() && heard.size >= words.size && heard.subList(0, words.size) == words
    }

    companion object {
        /**
         * The wake phrase, validated as the Rust core's `wake_phrase` does:
         * letters, spaces, apostrophes and hyphens (hyphens become spaces),
         * at most 80 characters. Null if invalid.
         */
        fun phrase(name: String, nameOnly: Boolean): String? {
            if (name.any { !(it in 'a'..'z' || it in 'A'..'Z' || it.isWhitespace() || it == '-' || it == '\'') }) return null
            val clean = name.replace('-', ' ').split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
            if (clean.isEmpty() || clean.length > 80) return null
            return if (nameOnly) clean else "Hey $clean"
        }

        private fun words(text: String): List<String> = text.lowercase(Locale.ROOT)
            .replace('-', ' ')
            .split(Regex("[^a-z']+"))
            .map { it.trim('\'') }
            .filter { it.isNotEmpty() }
    }
}

/**
 * Writes queued PCM to the recogniser's pipe on its own thread, so the
 * capture thread never blocks on the pipe. At most [maxBytes] are queued;
 * beyond that the oldest audio is dropped and counted. A write failure (the
 * recogniser closed its end) is reported once through [onFailure].
 */
class PcmPipeWriter(
    private val out: OutputStream,
    private val maxBytes: Int = DEFAULT_MAX_BYTES,
    private val onFailure: () -> Unit = {},
) : Closeable {
    private val lock = ReentrantLock()
    private val ready = lock.newCondition()
    private val queue = ArrayDeque<ByteArray>()
    private var queued = 0
    private var finishing = false
    private var closed = false
    @Volatile var droppedBytes = 0L
        private set

    private val thread = Thread({ run() }, "notdot-wake-pipe").apply {
        isDaemon = true
        start()
    }

    /** Queue a copy of [bytes]; never blocks. */
    fun offer(bytes: ByteArray): Boolean {
        if (bytes.isEmpty() || bytes.size % 2 != 0 || bytes.size > maxBytes) return false
        lock.withLock {
            if (closed || finishing) return false
            var dropped = false
            while (queued + bytes.size > maxBytes) {
                val old = queue.removeFirst()
                queued -= old.size
                droppedBytes += old.size
                dropped = true
            }
            queue.addLast(bytes.copyOf())
            queued += bytes.size
            ready.signalAll()
            return !dropped
        }
    }

    /** Write what is queued, then close the stream (end of audio). */
    fun finish() = lock.withLock {
        finishing = true
        ready.signalAll()
    }

    /** Discard what is queued and close the stream now. */
    override fun close() {
        lock.withLock {
            closed = true
            droppedBytes += queued
            queue.clear()
            queued = 0
            ready.signalAll()
        }
        // Unblocks a write in progress on the writer thread.
        runCatching { out.close() }
    }

    private fun run() {
        while (true) {
            val chunk = lock.withLock {
                while (queue.isEmpty() && !closed && !finishing) ready.await()
                if (closed || queue.isEmpty()) null else queue.removeFirst().also { queued -= it.size }
            } ?: break
            try {
                out.write(chunk)
            } catch (_: IOException) {
                val report = lock.withLock { !closed.also { closed = true } }
                if (report) onFailure()
                break
            }
        }
        runCatching { out.close() }
    }

    companion object {
        /** Two seconds of 16 kHz 16-bit mono audio. */
        const val DEFAULT_MAX_BYTES = 64_000
    }
}
