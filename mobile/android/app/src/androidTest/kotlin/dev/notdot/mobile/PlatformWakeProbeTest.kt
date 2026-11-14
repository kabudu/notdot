package dev.notdot.mobile

import android.content.Intent
import android.media.AudioFormat
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.speech.RecognitionListener
import android.speech.RecognitionSupport
import android.speech.RecognitionSupportCallback
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** Diagnostic SDK tests: always select the explicitly on-device service. */
@androidx.test.filters.SdkSuppress(minSdkVersion = 33)
@RunWith(AndroidJUnit4::class)
class PlatformWakeProbeTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    @Test
    fun inspectInstalledEnglishModels() {
        assumeTrue(android.os.Build.VERSION.SDK_INT >= 33 && SpeechRecognizer.isOnDeviceRecognitionAvailable(context))
        val done = CountDownLatch(1)
        var recognizer: SpeechRecognizer? = null
        instrumentation.runOnMainSync {
            recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            recognizer!!.checkRecognitionSupport(request("en-GB"), Executor { it.run() }, object : RecognitionSupportCallback {
                override fun onSupportResult(support: RecognitionSupport) {
                    val installed = support.installedOnDeviceLanguages
                    instrumentation.sendStatus(0, Bundle().apply {
                        putString("native_models", "installed=${installed.joinToString()} supported=${support.supportedOnDeviceLanguages.joinToString()} pending=${support.pendingOnDeviceLanguages.joinToString()}")
                    })
                    android.util.Log.i("notdot.test", "native_support english_gb=${installed.any { it.equals("en-GB", true) }} english_us=${installed.any { it.equals("en-US", true) }} installed_count=${installed.size}")
                    done.countDown()
                }
                override fun onError(error: Int) {
                    android.util.Log.i("notdot.test", "native_support_error=$error")
                    done.countDown()
                }
            })
        }
        try { assertTrue("support callback", done.await(10, TimeUnit.SECONDS)) }
        finally { instrumentation.runOnMainSync { recognizer?.destroy() } }
    }

    private fun request(language: String) = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
        .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
        .putExtra(RecognizerIntent.EXTRA_LANGUAGE, language)
        .putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        .putExtra(RecognizerIntent.EXTRA_ENABLE_BIASING_DEVICE_CONTEXT, false)
        .putStringArrayListExtra(RecognizerIntent.EXTRA_BIASING_STRINGS, arrayListOf("Hey Ava", "Ava"))
        .apply {
            when (InstrumentationRegistry.getArguments().getString("biased")) {
                "false" -> removeExtra(RecognizerIntent.EXTRA_BIASING_STRINGS)
                "balanced" -> putStringArrayListExtra(RecognizerIntent.EXTRA_BIASING_STRINGS,
                    arrayListOf("Hey Ava", "Hey Ada", "Hey Eva", "Hey Nova", "Ava", "Ada", "Eva", "Nova"))
            }
        }

    private fun pcm(path: String): ByteArray {
        val bytes = instrumentation.context.assets.open(path).use { it.readBytes() }
        var offset = 12
        while (offset + 8 <= bytes.size) {
            val size = ByteBuffer.wrap(bytes, offset + 4, 4).order(ByteOrder.LITTLE_ENDIAN).int
            if (String(bytes, offset, 4) == "data") return bytes.copyOfRange(offset + 8, offset + 8 + size)
            offset += 8 + size + (size and 1)
        }
        error("missing PCM data")
    }

    @Test
    fun adapterRecognisesPacedFixture() {
        val ready = CountDownLatch(1)
        val ended = CountDownLatch(1)
        val wakes = java.util.concurrent.atomic.AtomicInteger()
        val failure = AtomicReference<WakeRecognizerError?>(null)
        var adapter: OnDeviceWakeRecognizer? = null
        instrumentation.runOnMainSync {
            adapter = OnDeviceWakeRecognizer(context, object : OnDeviceWakeRecognizer.Listener {
                override fun onStatus(status: WakeRecognizerStatus) {
                    android.util.Log.i("notdot.test", "adapter_status=$status")
                    if (status is WakeRecognizerStatus.Listening) ready.countDown()
                    if (status is WakeRecognizerStatus.Ended) ended.countDown()
                }
                override fun onWake() { wakes.incrementAndGet() }
                override fun onError(error: WakeRecognizerError) { failure.set(error); ready.countDown(); ended.countDown() }
            }, language = "en-GB")
            adapter!!.start("Ava")
        }
        try {
            assertTrue("adapter ready", ready.await(10, TimeUnit.SECONDS))
            assertTrue("adapter startup: ${failure.get()}", failure.get() == null)
            val fixture = InstrumentationRegistry.getArguments().getString("fixture") ?: "fixtures/hey-ava.wav"
            val raw = pcm(fixture)
            val gain = (InstrumentationRegistry.getArguments().getString("gain")?.toDouble() ?: 1.0)
                .also { require(it.isFinite() && it in 0.0..1.0) }
            val samples = ByteBuffer.wrap(raw).order(ByteOrder.LITTLE_ENDIAN)
            val scaled = ByteBuffer.allocate(raw.size).order(ByteOrder.LITTLE_ENDIAN)
            while (samples.remaining() >= 2) {
                scaled.putShort((samples.short * gain).toInt().coerceIn(-32768, 32767).toShort())
            }
            val audio = ByteArray(16_000) + scaled.array() + ByteArray(48_000)
            val started = System.nanoTime()
            for (offset in audio.indices step 640) {
                adapter!!.feedPcm16(audio.copyOfRange(offset, minOf(offset + 640, audio.size)))
                Thread.sleep(20)
            }
            instrumentation.runOnMainSync { adapter!!.finishAudio() }
            assertTrue("adapter ended", ended.await(10, TimeUnit.SECONDS))
            instrumentation.runOnMainSync {
                instrumentation.sendStatus(0, Bundle().apply {
                    putString("adapter_result", "wakes=${wakes.get()} failure=${failure.get()} elapsed_ms=${(System.nanoTime()-started)/1e6} stats=${adapter!!.stats()}")
                })
            }
            val expectedWakes = if (InstrumentationRegistry.getArguments().getString("expected") == "false") 0 else 1
            assertTrue("expected $expectedWakes wakes, got ${wakes.get()}; error=${failure.get()}",
                wakes.get() == expectedWakes && failure.get() == null)
        } finally { instrumentation.runOnMainSync { adapter?.stop() } }
    }

    @Test
    fun inspectNativeHypothesesForSuppliedFixture() {
        assumeTrue(android.os.Build.VERSION.SDK_INT >= 33 && SpeechRecognizer.isOnDeviceRecognitionAvailable(context))
        val done = CountDownLatch(1)
        val result = AtomicReference<List<String>>(emptyList())
        val failure = AtomicReference<Int?>(null)
        val writerFailed = java.util.concurrent.atomic.AtomicBoolean(false)
        val sourceSpeechEnd = java.util.concurrent.atomic.AtomicLong(0)
        val confidence = AtomicReference<FloatArray?>(null)
        val wokeAt = java.util.concurrent.atomic.AtomicLong(0)
        val pipe = ParcelFileDescriptor.createPipe()
        var recognizer: SpeechRecognizer? = null
        val started = System.nanoTime()
        instrumentation.runOnMainSync {
            recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            recognizer!!.setRecognitionListener(object : RecognitionListener {
                override fun onReadyForSpeech(params: Bundle?) {}
                override fun onBeginningOfSpeech() {}
                override fun onRmsChanged(rmsdB: Float) {}
                override fun onBufferReceived(buffer: ByteArray?) {}
                override fun onEndOfSpeech() {}
                override fun onPartialResults(results: Bundle?) {}
                override fun onSegmentResults(results: Bundle) {
                    result.set(results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION).orEmpty())
                    confidence.set(results.getFloatArray(SpeechRecognizer.CONFIDENCE_SCORES))
                    if (WakePhraseMatcher("Hey Ava").matches(result.get().firstOrNull().orEmpty())) wokeAt.compareAndSet(0, System.nanoTime())
                }
                override fun onEndOfSegmentedSession() { done.countDown() }
                override fun onEvent(eventType: Int, params: Bundle?) {}
                override fun onError(error: Int) { failure.set(error); done.countDown() }
                override fun onResults(results: Bundle?) {
                    val heard = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION).orEmpty()
                    if (heard.isNotEmpty()) {
                        result.set(heard); confidence.set(results?.getFloatArray(SpeechRecognizer.CONFIDENCE_SCORES))
                        if (WakePhraseMatcher("Hey Ava").matches(heard.first())) wokeAt.compareAndSet(0, System.nanoTime())
                    }
                    done.countDown()
                }
            })
            recognizer!!.startListening(request("en-GB")
                .putExtra(RecognizerIntent.EXTRA_SEGMENTED_SESSION, RecognizerIntent.EXTRA_AUDIO_SOURCE)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE, pipe[0])
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_CHANNEL_COUNT, 1)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_SAMPLING_RATE, 16_000))
        }
        val writer = Thread {
            try { ParcelFileDescriptor.AutoCloseOutputStream(pipe[1]).use { out ->
                val fixture = InstrumentationRegistry.getArguments().getString("fixture") ?: "fixtures/hey-ava.wav"
                val raw = pcm(fixture)
                val gain = (InstrumentationRegistry.getArguments().getString("gain")?.toDouble() ?: 1.0).also { require(it.isFinite() && it in 0.0..1.0) }
                val samples = ByteBuffer.wrap(raw).order(ByteOrder.LITTLE_ENDIAN)
                val scaled = ByteBuffer.allocate(raw.size).order(ByteOrder.LITTLE_ENDIAN)
                while (samples.remaining() >= 2) scaled.putShort((samples.short * gain).toInt().coerceIn(-32768, 32767).toShort())
                val idle = (InstrumentationRegistry.getArguments().getString("idleSeconds")?.toInt() ?: 0).also { require(it in 0..60) }
                val prefix = 16_000 + idle * 32_000
                val shorts = ByteBuffer.wrap(scaled.array()).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
                var lastVoice = 0
                var peak = 0
                for (i in 0 until shorts.limit()) peak = maxOf(peak, kotlin.math.abs(shorts[i].toInt()))
                for (i in 0 until shorts.limit()) if (kotlin.math.abs(shorts[i].toInt()) > peak / 100) lastVoice = i
                val audio = ByteArray(prefix) + scaled.array() + ByteArray(48_000)
                for (offset in audio.indices step 640) {
                    out.write(audio, offset, minOf(640, audio.size-offset))
                    if (offset <= prefix + lastVoice * 2 && offset + 640 > prefix + lastVoice * 2)
                        sourceSpeechEnd.set(System.nanoTime())
                    Thread.sleep(20)
                }
            } } catch (_: Exception) { writerFailed.set(true); done.countDown() }
        }.apply { start() }
        try {
            assertTrue("recognition callback", done.await((InstrumentationRegistry.getArguments().getString("idleSeconds")?.toLong() ?: 0L) + 20L, TimeUnit.SECONDS))
            val woke = result.get().any { text -> text.lowercase().replace(Regex("[^a-z ]"), "").trim() == "hey ava" }
            android.util.Log.i("notdot.test", "native_fixture woke=$woke error=${failure.get()} result_count=${result.get().size} elapsed_ms=${(System.nanoTime() - started) / 1e6}")
            instrumentation.sendStatus(0, Bundle().apply {
                val top = result.get().firstOrNull().orEmpty()
                val words = top.lowercase().split(Regex("[^a-z]+"))
                val category = listOf("ava", "ada", "eva", "ever", "either", "nova").firstOrNull { it in words } ?: "other"
                val power = context.getSystemService(android.os.PowerManager::class.java)
                val locked = context.getSystemService(android.app.KeyguardManager::class.java).isKeyguardLocked
                val delayMs = if (wokeAt.get() == 0L) -1 else (wokeAt.get()-sourceSpeechEnd.get())/1_000_000
                putString("native_timing", "speech_end_to_top_wake_ms=$delayMs interactive=${power.isInteractive} keyguard_locked=$locked")
                putString("native_result", "woke=$woke top_wake=${WakePhraseMatcher("Hey Ava").matches(top)} top_name=$category top_words=${words.size} error=${failure.get()} writer_failed=${writerFailed.get()} count=${result.get().size} confidence=${confidence.get()?.joinToString()}")
            })
            val expected = InstrumentationRegistry.getArguments().getString("expected") != "false"
            assertTrue("on-device fixture result expected=$expected woke=$woke; error=${failure.get()}", woke == expected && failure.get() == null && !writerFailed.get())
        } finally {
            instrumentation.runOnMainSync { recognizer?.cancel(); recognizer?.destroy() }
            runCatching { pipe[0].close() }; runCatching { pipe[1].close() }
            writer.join(1_000)
        }
    }
}
