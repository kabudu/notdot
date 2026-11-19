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
 * (`SpeechRecognizer.createOnDeviceSpeechRecognizer`), fed audio the app
 * has already captured. No model is bundled and there is no cloud fallback:
 * `createSpeechRecognizer` is never used, and listening starts only when the
 * language is installed on the device. Nothing is downloaded.
 *
 * Contract (the logic lives in [WakeRecognizerCore]):
 * - [start], [finishAudio], [stop] and [stats] are called on the main
 *   thread, and every [Listener] callback arrives there. [feedPcm16] may be
 *   called from any thread and never blocks (at most two seconds queued;
 *   older audio is dropped and counted).
 * - Audio is 16-bit little-endian PCM, 16 kHz, mono; the caller converts.
 * - A wake is a final segment whose best hypothesis starts with the whole
 *   phrase ([WakePhraseMatcher]). Partial results never wake, and the
 *   session's closing result never repeats a segment's wake.
 * - The language must be listed as installed, unless the service cannot
 *   say (an error, or all lists empty): then listening starts unverified.
 * - The language check and the recogniser's start are each bounded
 *   ([WakeRecognizerCore.SUPPORT_TIMEOUT_MS], [WakeRecognizerCore.READY_TIMEOUT_MS]).
 * - Nothing restarts automatically. A session ends with [Listener.onStatus]
 *   `Ended` or [Listener.onError]; the caller restarts, if at all, as
 *   [WakeRestartPolicy] describes.
 * - After [stop] or a new [start] (a name change, say), nothing from the
 *   earlier session is delivered or can affect the new one, even when a
 *   listener callback itself stops or restarts.
 * - Speech and transcripts are never stored or logged; [stats] holds counts.
 *
 * Needs Android 13 (API 33): app-supplied audio, segmented sessions and
 * biasing strings. The foreground microphone service and RECORD_AUDIO
 * permission remain the caller's responsibility.
 */
class OnDeviceWakeRecognizer(
    context: Context,
    listener: Listener,
    language: String = "en-US",
) {
    interface Listener {
        fun onStatus(status: WakeRecognizerStatus)
        fun onWake()
        fun onError(error: WakeRecognizerError)
    }

    private val core = WakeRecognizerCore(
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) AndroidRecognizerPort(context) else null,
        MainScheduler(),
        listener,
        language,
    )

    fun start(name: String, nameOnly: Boolean = false) = core.start(name, nameOnly)
    fun feedPcm16(bytes: ByteArray): Boolean = core.feedPcm16(bytes)
    fun finishAudio() = core.finishAudio()
    fun stop() = core.stop()
    fun stats(): WakeRecognizerStats = core.stats()
}

sealed interface WakeRecognizerStatus {
    data object CheckingLanguage : WakeRecognizerStatus
    /**
     * `languageVerified` is false when the service could not report support,
     * or reported nothing (all lists empty); a missing language then shows
     * as `Recognizer` error 12 or 13.
     */
    data class Listening(val languageVerified: Boolean) : WakeRecognizerStatus
    /** The session ended: the audio finished, or the service ended it. */
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
    /** The service gave no sign of life in time ("language check" or "start"). */
    data class Timeout(val stage: String) : WakeRecognizerError
    /** The platform refused a call (for example a missing permission). */
    data class Platform(val reason: String) : WakeRecognizerError
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

/** What the recogniser service reports for a language. */
data class LanguageSupport(val installed: List<String>, val supported: List<String>, val pending: List<String>)

/** Events from one recogniser session, already unpacked from Bundles. */
interface RecognizerEvents {
    /** The service is alive: ready, hearing speech, or reporting levels. */
    fun onReady()
    fun onPartial()
    fun onSegment(best: String?)
    fun onResults(best: String?)
    fun onEndOfSession()
    fun onError(code: Int)
}

/**
 * The platform recogniser, one session at a time. Calls may throw (a
 * missing permission, a dead service); the core treats that as a failure.
 */
interface RecognizerPort {
    fun available(): Boolean
    /** Create the recogniser and route its events to [events]. */
    fun create(events: RecognizerEvents)
    fun checkSupport(language: String, phrase: String, onResult: (LanguageSupport) -> Unit, onError: (Int) -> Unit)
    /** Start listening to app audio; returns the stream to write PCM to. */
    fun start(language: String, phrase: String): OutputStream
    /** Cancel and destroy the recogniser and close the audio source. */
    fun release()
}

/** Main-thread scheduling, so tests can drive time. */
interface WakeScheduler {
    fun isMainThread(): Boolean
    fun post(action: () -> Unit)
    fun postDelayed(delayMs: Long, token: Any, action: () -> Unit)
    fun cancel(token: Any)
}

/**
 * The adapter's state machine, free of Android types. Every method except
 * [feedPcm16] runs on the main thread; every port and timer callback is
 * tagged with its session and ignored once that session is over.
 */
class WakeRecognizerCore(
    private val port: RecognizerPort?,
    private val scheduler: WakeScheduler,
    private val listener: OnDeviceWakeRecognizer.Listener,
    private val language: String,
    private val writerFactory: (OutputStream, () -> Unit) -> PcmPipeWriter = { out, failed -> PcmPipeWriter(out, onFailure = failed) },
) {
    private var session = 0L
    private var active = false
    private var matcher: WakePhraseMatcher? = null
    @Volatile private var writer: PcmPipeWriter? = null
    private var counts = WakeRecognizerStats()
    /** Cancels this session's timers (schedulers match tokens by identity). */
    private var timers = Any()

    fun stats(): WakeRecognizerStats {
        requireMain()
        return counts.copy(droppedBytes = counts.droppedBytes + (writer?.droppedBytes ?: 0))
    }

    fun start(name: String, nameOnly: Boolean = false) {
        requireMain()
        stop()
        val phrase = WakePhraseMatcher.phrase(name, nameOnly) ?: return listener.onError(WakeRecognizerError.InvalidName)
        if (port == null) {
            return listener.onError(WakeRecognizerError.Unsupported("Android 13 or later is needed to recognise app-supplied audio on the device"))
        }
        val id = session
        val unavailable = WakeRecognizerError.Unsupported("This device has no on-device speech recogniser")
        if (!guard(id) { port.available() }.let { it == true }) {
            if (id == session) fail(id, unavailable)
            return
        }
        active = true
        matcher = WakePhraseMatcher(phrase)
        counts = WakeRecognizerStats()
        if (guard(id) { port.create(Events(id)) } == null) return
        listener.onStatus(WakeRecognizerStatus.CheckingLanguage)
        if (id != session) return
        scheduler.postDelayed(SUPPORT_TIMEOUT_MS, timers) { if (id == session) fail(id, WakeRecognizerError.Timeout("language check")) }
        guard(id) {
            port.checkSupport(
                language,
                phrase,
                onResult = { support -> scheduler.post { supportResult(id, phrase, support) } },
                onError = { code -> scheduler.post { supportError(id, phrase, code) } },
            )
        }
    }

    fun feedPcm16(bytes: ByteArray): Boolean = writer?.offer(bytes) ?: false

    fun finishAudio() {
        requireMain()
        writer?.finish()
    }

    fun stop() {
        requireMain()
        end(session)
    }

    private fun supportResult(id: Long, phrase: String, support: LanguageSupport) {
        if (id != session) return
        scheduler.cancel(timers)
        if (support.installed.any { it.sameLanguage(language) }) {
            listen(id, phrase, verified = true)
        } else if (support.installed.isEmpty() && support.supported.isEmpty() && support.pending.isEmpty()) {
            // All lists empty: the reference defines each list only by what
            // it contains, not an empty answer, and a provider has been seen
            // to answer this way yet recognise. Treat it as unknown, like
            // ERROR_CANNOT_CHECK_SUPPORT: start, and let the recogniser report
            // ERROR_LANGUAGE_* if the language is really missing.
            listen(id, phrase, verified = false)
        } else {
            fail(
                id,
                WakeRecognizerError.LanguageNotInstalled(
                    language,
                    downloadable = support.supported.any { it.sameLanguage(language) },
                    pending = support.pending.any { it.sameLanguage(language) },
                ),
            )
        }
    }

    private fun supportError(id: Long, phrase: String, code: Int) {
        if (id != session) return
        scheduler.cancel(timers)
        // Some services cannot answer; the start then reports a missing
        // language itself (ERROR_LANGUAGE_*).
        if (code == CANNOT_CHECK_SUPPORT) listen(id, phrase, verified = false) else fail(id, WakeRecognizerError.Recognizer(code))
    }

    private fun listen(id: Long, phrase: String, verified: Boolean) {
        val out = guard(id) { port!!.start(language, phrase) } ?: return
        writer = writerFactory(out) { scheduler.post { if (id == session) fail(id, WakeRecognizerError.AudioPipe) } }
        scheduler.postDelayed(READY_TIMEOUT_MS, timers) { if (id == session) fail(id, WakeRecognizerError.Timeout("start")) }
        listener.onStatus(WakeRecognizerStatus.Listening(languageVerified = verified))
    }

    /** Run a port call; on an exception end the session and report it. */
    private fun <T> guard(id: Long, call: () -> T): T? = try {
        call()
    } catch (e: SecurityException) {
        fail(id, WakeRecognizerError.Platform("Not permitted: ${e.javaClass.simpleName}"))
        null
    } catch (e: RuntimeException) {
        // IllegalStateException, IllegalArgumentException, Unsupported..., or
        // a dead service: never crash the capture service over the recogniser.
        fail(id, WakeRecognizerError.Platform("Recogniser call failed: ${e.javaClass.simpleName}"))
        null
    } catch (_: IOException) {
        fail(id, WakeRecognizerError.AudioPipe)
        null
    }

    /** End session [id] if it is still current. True if it was. */
    private fun end(id: Long): Boolean {
        if (id != session) return false
        session++
        scheduler.cancel(timers)
        timers = Any()
        writer?.let {
            counts = counts.copy(droppedBytes = counts.droppedBytes + it.droppedBytes)
            it.close()
        }
        writer = null
        matcher = null
        if (active) {
            active = false
            runCatching { port?.release() }
        }
        return true
    }

    private fun fail(id: Long, error: WakeRecognizerError) {
        if (end(id)) listener.onError(error)
    }

    private fun ended(id: Long, reason: String) {
        if (end(id)) listener.onStatus(WakeRecognizerStatus.Ended(reason))
    }

    private fun requireMain() = check(scheduler.isMainThread()) { "OnDeviceWakeRecognizer must be used on the main thread" }

    private inner class Events(private val id: Long) : RecognizerEvents {
        private var wokeInSession = false
        private val current get() = id == session

        override fun onReady() {
            if (current) scheduler.cancel(timers)
        }

        override fun onPartial() {
            if (!current) return
            scheduler.cancel(timers)
            // Never a wake: a partial name may still change.
            counts = counts.copy(partials = counts.partials + 1)
        }

        override fun onSegment(best: String?) {
            if (!current) return
            scheduler.cancel(timers)
            counts = counts.copy(segments = counts.segments + 1)
            wake(best)
        }

        override fun onResults(best: String?) {
            if (!current) return
            // The closing result repeats speech already given as segments:
            // it can wake only if no segment has.
            if (!wokeInSession) wake(best)
            if (current) ended(id, "results")
        }

        override fun onEndOfSession() {
            if (current) ended(id, "audio ended")
        }

        override fun onError(code: Int) {
            if (current) fail(id, WakeRecognizerError.Recognizer(code))
        }

        private fun wake(best: String?) {
            if (best != null && matcher?.matches(best) == true) {
                wokeInSession = true
                counts = counts.copy(wakes = counts.wakes + 1)
                // May stop or restart the recogniser; nothing here runs after.
                listener.onWake()
            }
        }
    }

    companion object {
        const val SUPPORT_TIMEOUT_MS = 5_000L
        const val READY_TIMEOUT_MS = 5_000L
        const val CANNOT_CHECK_SUPPORT = 14

        private fun String.sameLanguage(tag: String) = replace('_', '-').equals(tag.replace('_', '-'), ignoreCase = true)
    }
}

/**
 * When the caller should start again after a session ends. The official
 * reference says SpeechRecognizer "is not intended to be used for continuous
 * recognition", and a provider may end a segmented session at any time, so a
 * long listen is a series of sessions:
 * - restart after [WakeRecognizerStatus.Ended] or a [retryable] error, after
 *   [nextDelayMs]; never on a terminal error (permission, language, name,
 *   unsupported device), which needs the user or a setting to change;
 * - delays double from 1 s to 60 s, and reset once a session has listened
 *   for a minute, so a provider that ends sessions immediately is retried
 *   at most once a minute rather than in a loop.
 */
class WakeRestartPolicy(
    private val initialMs: Long = 1_000,
    private val maxMs: Long = 60_000,
    private val stableMs: Long = 60_000,
) {
    private var delay = initialMs

    /** The wait before restarting, given how long the session listened. */
    fun nextDelayMs(sessionMs: Long): Long {
        if (sessionMs >= stableMs) delay = initialMs
        val now = delay
        delay = (delay * 2).coerceAtMost(maxMs)
        return now
    }

    companion object {
        fun retryable(error: WakeRecognizerError): Boolean = when (error) {
            is WakeRecognizerError.Recognizer -> error.code !in setOf(9, 12, 13)
            is WakeRecognizerError.Timeout, WakeRecognizerError.AudioPipe -> true
            is WakeRecognizerError.Platform, is WakeRecognizerError.Unsupported,
            is WakeRecognizerError.LanguageNotInstalled, WakeRecognizerError.InvalidName -> false
        }
    }
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

    init {
        Thread({ run() }, "notdot-wake-pipe").apply {
            isDaemon = true
            start()
        }
    }

    /** Queue a copy of [bytes]; never blocks. False if anything was dropped or refused. */
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

private class MainScheduler : WakeScheduler {
    private val handler = Handler(Looper.getMainLooper())
    override fun isMainThread() = Looper.myLooper() == Looper.getMainLooper()
    override fun post(action: () -> Unit) {
        handler.post { action() }
    }
    override fun postDelayed(delayMs: Long, token: Any, action: () -> Unit) {
        handler.postDelayed({ action() }, token, delayMs)
    }
    override fun cancel(token: Any) = handler.removeCallbacksAndMessages(token)
}

/** The OS on-device recogniser, fed through a pipe. API 33+. */
@RequiresApi(Build.VERSION_CODES.TIRAMISU)
private class AndroidRecognizerPort(private val context: Context) : RecognizerPort {
    private var recognizer: SpeechRecognizer? = null
    private var source: ParcelFileDescriptor? = null

    override fun available() = SpeechRecognizer.isOnDeviceRecognitionAvailable(context)

    override fun create(events: RecognizerEvents) {
        val created = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        recognizer = created
        created.setRecognitionListener(object : RecognitionListener {
            override fun onReadyForSpeech(params: Bundle?) = events.onReady()
            override fun onPartialResults(partialResults: Bundle?) = events.onPartial()
            override fun onSegmentResults(segmentResults: Bundle) = events.onSegment(best(segmentResults))
            override fun onResults(results: Bundle?) = events.onResults(best(results))
            override fun onEndOfSegmentedSession() = events.onEndOfSession()
            override fun onError(error: Int) = events.onError(error)
            // Any sign of life ends the start timeout: some providers may
            // not send onReadyForSpeech for an app-supplied source.
            override fun onBeginningOfSpeech() = events.onReady()
            override fun onRmsChanged(rmsdB: Float) = events.onReady()
            override fun onBufferReceived(buffer: ByteArray?) = Unit
            override fun onEndOfSpeech() = Unit
            override fun onEvent(eventType: Int, params: Bundle?) = Unit
        })
    }

    private fun best(bundle: Bundle?) = bundle?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()

    override fun checkSupport(language: String, phrase: String, onResult: (LanguageSupport) -> Unit, onError: (Int) -> Unit) {
        recognizer!!.checkRecognitionSupport(
            intent(language, phrase),
            context.mainExecutor,
            object : RecognitionSupportCallback {
                override fun onSupportResult(support: RecognitionSupport) = onResult(
                    LanguageSupport(support.installedOnDeviceLanguages, support.supportedOnDeviceLanguages, support.pendingOnDeviceLanguages),
                )
                override fun onError(error: Int) = onError(error)
            },
        )
    }

    override fun start(language: String, phrase: String): OutputStream {
        val pipe = ParcelFileDescriptor.createPipe()
        source = pipe[0]
        val out = ParcelFileDescriptor.AutoCloseOutputStream(pipe[1])
        try {
            recognizer!!.startListening(
                intent(language, phrase).apply {
                    putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE, pipe[0])
                    putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
                    putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_SAMPLING_RATE, 16_000)
                    putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_CHANNEL_COUNT, 1)
                    // One session until the audio is closed, reported per segment.
                    putExtra(RecognizerIntent.EXTRA_SEGMENTED_SESSION, RecognizerIntent.EXTRA_AUDIO_SOURCE)
                    putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
                },
            )
        } catch (e: RuntimeException) {
            runCatching { out.close() }
            throw e
        }
        return out
    }

    private fun intent(language: String, phrase: String) = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
        putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
        putExtra(RecognizerIntent.EXTRA_LANGUAGE, language)
        putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
        putStringArrayListExtra(RecognizerIntent.EXTRA_BIASING_STRINGS, arrayListOf(phrase))
        putExtra(RecognizerIntent.EXTRA_ENABLE_BIASING_DEVICE_CONTEXT, false)
    }

    override fun release() {
        recognizer?.let {
            runCatching { it.cancel() }
            runCatching { it.destroy() }
        }
        recognizer = null
        runCatching { source?.close() }
        source = null
    }
}
