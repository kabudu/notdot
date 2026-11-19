package dev.notdot.mobile

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.OutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** JVM tests of the on-device wake adapter's framework-free parts. */
class OnDeviceWakeRecognizerTest {
    @Test
    fun phrasesAreValidatedLikeTheCore() {
        assertEquals("Hey Ava", WakePhraseMatcher.phrase("Ava", false))
        assertEquals("Ava", WakePhraseMatcher.phrase(" Ava ", true))
        assertEquals("Hey Mary Jane", WakePhraseMatcher.phrase("Mary-Jane", false))
        assertEquals("Hey O'Neil", WakePhraseMatcher.phrase("O'Neil", false))
        assertNull(WakePhraseMatcher.phrase("Ava #0.01", false))
        assertNull(WakePhraseMatcher.phrase("Zoë", false))
        assertNull(WakePhraseMatcher.phrase("   ", false))
        assertNull(WakePhraseMatcher.phrase("a".repeat(81), false))
    }

    @Test
    fun onlyTheWholePhraseAtTheStartWakes() {
        val ava = WakePhraseMatcher("Hey Ava")
        for (yes in listOf("Hey Ava", "hey, ava.", "Hey Ava, what's on my calendar today?", "HEY AVA!")) {
            assertTrue(yes, ava.matches(yes))
        }
        for (no in listOf(
            "Hey Eva", "Hey Ada", "Hey Nova", "Hey Avalon", "Hey Aver", "Ava", "Hey", "Hey A",
            "Ava said hey to everyone", "Okay hey Ava", "Have a nice day", "", "hey ava".substring(0, 6),
        )) {
            assertFalse(no, ava.matches(no))
        }
        val nameOnly = WakePhraseMatcher("Ava")
        assertTrue(nameOnly.matches("Ava, play music"))
        assertFalse(nameOnly.matches("Avalon"))
        val apostrophe = WakePhraseMatcher("Hey O'Neil")
        assertTrue(apostrophe.matches("hey o'neil"))
        assertFalse(apostrophe.matches("hey o neil"))
    }

    @Test
    fun theWriterKeepsOrderAndFinishesTheStream() {
        val out = ByteArrayOutputStream()
        val closed = CountDownLatch(1)
        val stream = object : OutputStream() {
            override fun write(b: Int) = out.write(b)
            override fun write(b: ByteArray, off: Int, len: Int) = out.write(b, off, len)
            override fun close() = closed.countDown()
        }
        val writer = PcmPipeWriter(stream)
        assertTrue(writer.offer(byteArrayOf(1, 2)))
        assertTrue(writer.offer(byteArrayOf(3, 4, 5, 6)))
        assertFalse("odd bytes are not whole samples", writer.offer(byteArrayOf(7)))
        assertFalse(writer.offer(ByteArray(0)))
        writer.finish()
        assertFalse("no audio after finish", writer.offer(byteArrayOf(8, 9)))
        assertTrue(closed.await(5, TimeUnit.SECONDS))
        assertArrayEquals(byteArrayOf(1, 2, 3, 4, 5, 6), out.toByteArray())
        assertEquals(0, writer.droppedBytes)
    }

    /** A stalled reader never blocks the caller: the oldest audio is dropped. */
    @Test
    fun aStalledPipeDropsTheOldestAudioWithoutBlocking() {
        val release = CountDownLatch(1)
        val written = ByteArrayOutputStream()
        val stream = object : OutputStream() {
            override fun write(b: Int) = throw UnsupportedOperationException()
            override fun write(b: ByteArray, off: Int, len: Int) {
                release.await()
                synchronized(written) { written.write(b, off, len) }
            }
        }
        val writer = PcmPipeWriter(stream, maxBytes = 8)
        val started = System.nanoTime()
        writer.offer(byteArrayOf(0, 0)) // taken by the writer, which then blocks
        Thread.sleep(50)
        for (i in 1..10) writer.offer(byteArrayOf(i.toByte(), i.toByte()))
        assertTrue("offer must not block", TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started) < 1_000)
        assertEquals(12L, writer.droppedBytes)
        writer.finish()
        release.countDown()
        Thread.sleep(200)
        val bytes = synchronized(written) { written.toByteArray() }
        assertArrayEquals(byteArrayOf(0, 0, 7, 7, 8, 8, 9, 9, 10, 10), bytes)
    }

    @Test
    fun aClosedReaderIsReportedOnceAndCloseDiscardsQueuedAudio() {
        val failures = AtomicInteger()
        val failed = CountDownLatch(1)
        val broken = object : OutputStream() {
            override fun write(b: Int) = throw IOException("pipe closed")
            override fun write(b: ByteArray, off: Int, len: Int) = throw IOException("pipe closed")
        }
        val writer = PcmPipeWriter(broken) {
            failures.incrementAndGet()
            failed.countDown()
        }
        writer.offer(byteArrayOf(1, 2))
        assertTrue(failed.await(5, TimeUnit.SECONDS))
        assertFalse(writer.offer(byteArrayOf(3, 4)))
        writer.close()
        Thread.sleep(50)
        assertEquals(1, failures.get())

        // close() is not a failure, and drops what was waiting.
        val gate = CountDownLatch(1)
        val slow = object : OutputStream() {
            override fun write(b: Int) = Unit
            override fun write(b: ByteArray, off: Int, len: Int) {
                gate.await()
            }
            override fun close() = gate.countDown()
        }
        val quiet = AtomicInteger()
        val second = PcmPipeWriter(slow) { quiet.incrementAndGet() }
        second.offer(byteArrayOf(1, 2))
        Thread.sleep(50)
        second.offer(byteArrayOf(3, 4))
        second.close()
        Thread.sleep(50)
        assertEquals(2L, second.droppedBytes)
        assertEquals(0, quiet.get())
    }

    // ---- Session logic, with a fake recogniser and a manual clock. ----

    private class Clock : WakeScheduler {
        var now = 0L
        private val due = mutableListOf<Triple<Long, Any?, () -> Unit>>()
        override fun isMainThread() = true
        override fun post(action: () -> Unit) {
            due += Triple(now, null, action)
        }
        override fun postDelayed(delayMs: Long, token: Any, action: () -> Unit) {
            due += Triple(now + delayMs, token, action)
        }
        override fun cancel(token: Any) {
            due.removeAll { it.second === token }
        }
        fun advance(ms: Long = 0) {
            now += ms
            while (true) {
                val next = due.filter { it.first <= now }.minByOrNull { it.first } ?: break
                due.remove(next)
                next.third()
            }
        }
    }

    private class FakePort(
        var installed: List<String> = listOf("en-US"),
        var throwOnStart: RuntimeException? = null,
        var answerSupport: Boolean = true,
        var supported: List<String> = listOf("en-US", "fr-FR"),
    ) : RecognizerPort {
        var events: RecognizerEvents? = null
        val created = mutableListOf<RecognizerEvents>()
        var releases = 0
        var starts = 0
        val streams = mutableListOf<ByteArrayOutputStream>()
        override fun available() = true
        override fun create(events: RecognizerEvents) {
            this.events = events
            created += events
        }
        override fun checkSupport(language: String, phrase: String, onResult: (LanguageSupport) -> Unit, onError: (Int) -> Unit) {
            if (answerSupport) onResult(LanguageSupport(installed, supported, emptyList()))
        }
        override fun start(language: String, phrase: String): OutputStream {
            throwOnStart?.let { throw it }
            starts++
            return ByteArrayOutputStream().also { streams += it }
        }
        override fun release() {
            releases++
        }
    }

    private class Recorder : OnDeviceWakeRecognizer.Listener {
        val log = mutableListOf<String>()
        var onWakeAction: () -> Unit = {}
        override fun onStatus(status: WakeRecognizerStatus) {
            log += status.toString()
        }
        override fun onWake() {
            log += "wake"
            onWakeAction()
        }
        override fun onError(error: WakeRecognizerError) {
            log += "error $error"
        }
    }

    private fun setup(port: FakePort = FakePort()): Triple<WakeRecognizerCore, FakePort, Pair<Clock, Recorder>> {
        val clock = Clock()
        val recorder = Recorder()
        val core = WakeRecognizerCore(port, clock, recorder, "en-US")
        return Triple(core, port, clock to recorder)
    }

    @Test
    fun aListeningSessionWakesOncePerMatchingSegmentAndNeverOnPartials() {
        val (core, port, pair) = setup()
        val (clock, rec) = pair
        core.start("Ava")
        clock.advance()
        assertEquals(listOf("CheckingLanguage", "Listening(languageVerified=true)"), rec.log)
        val events = port.events!!
        events.onReady()
        events.onPartial()
        events.onSegment("Hey Eva")
        events.onSegment("Hey Ava")
        // The closing result repeats the segments: no second wake.
        events.onResults("Hey Ava")
        assertEquals(1, rec.log.count { it == "wake" })
        assertEquals("Ended(reason=results)", rec.log.last())
        assertEquals(WakeRecognizerStats(segments = 2, partials = 1, wakes = 1), core.stats())
        assertEquals(1, port.releases)
    }

    @Test
    fun aRestartInsideOnWakeIsNotReleasedByTheOldSession() {
        val (core, port, pair) = setup()
        val (clock, rec) = pair
        core.start("Ava")
        clock.advance()
        val old = port.events!!
        rec.onWakeAction = { core.start("Nova") } // e.g. a name change on wake
        old.onSegment("Hey Ava")
        rec.onWakeAction = {}
        old.onResults("Hey Ava") // stale: must not end or release the new session
        old.onEndOfSession()
        old.onError(7)
        clock.advance()
        val new = port.events!!
        assertTrue(new !== old)
        assertEquals(1, port.releases) // only the old session
        assertEquals(2, port.starts)
        assertFalse(rec.log.any { it.startsWith("Ended") || it.startsWith("error") })
        new.onSegment("Hey Ava")
        new.onSegment("Hey Nova")
        assertEquals(2, rec.log.count { it == "wake" })
        assertTrue(core.feedPcm16(byteArrayOf(1, 2)))
    }

    /** A wake from the closing result that restarts must not end the restart. */
    @Test
    fun aRestartInsideAClosingResultWakeKeepsTheNewSession() {
        val (core, port, pair) = setup()
        val (clock, rec) = pair
        core.start("Ava")
        clock.advance()
        val old = port.events!!
        rec.onWakeAction = { core.start("Ava") }
        old.onResults("Hey Ava")
        rec.onWakeAction = {}
        clock.advance()
        assertEquals(1, rec.log.count { it == "wake" })
        assertFalse(rec.log.any { it.startsWith("Ended") })
        assertEquals(1, port.releases)
        assertEquals(2, port.starts)
        assertTrue(core.feedPcm16(byteArrayOf(1, 2)))
    }

    @Test
    fun aStopInsideOnWakeEndsQuietlyAndLaterCallbacksAreIgnored() {
        val (core, port, pair) = setup()
        val (clock, rec) = pair
        core.start("Ava")
        clock.advance()
        val events = port.events!!
        rec.onWakeAction = { core.stop() }
        events.onSegment("Hey Ava")
        events.onSegment("Hey Ava")
        events.onError(3)
        clock.advance(60_000)
        assertEquals(1, rec.log.count { it == "wake" })
        assertFalse(rec.log.any { it.startsWith("error") || it.startsWith("Ended") })
        assertEquals(1, port.releases)
        assertFalse(core.feedPcm16(byteArrayOf(1, 2)))
    }

    @Test
    fun theLanguageCheckAndStartAreBounded() {
        val silent = FakePort(answerSupport = false)
        val (core, port, pair) = setup(silent)
        val (clock, rec) = pair
        core.start("Ava")
        clock.advance(WakeRecognizerCore.SUPPORT_TIMEOUT_MS)
        assertEquals("error Timeout(stage=language check)", rec.log.last())
        assertEquals(1, port.releases)

        // Started, but the service never shows a sign of life.
        val (core2, port2, pair2) = setup()
        core2.start("Ava")
        pair2.first.advance()
        pair2.first.advance(WakeRecognizerCore.READY_TIMEOUT_MS - 1)
        assertFalse(pair2.second.log.any { it.startsWith("error") })
        pair2.first.advance(1)
        assertEquals("error Timeout(stage=start)", pair2.second.log.last())
        assertEquals(1, port2.releases)

        // Any sign of life (here a level report) clears the start timeout.
        val (core3, port3, pair3) = setup()
        core3.start("Ava")
        pair3.first.advance()
        port3.events!!.onReady()
        pair3.first.advance(60_000)
        assertFalse(pair3.second.log.any { it.startsWith("error") })
    }

    @Test
    fun platformExceptionsAndMissingLanguagesEndTheSessionWithoutCrashing() {
        val denied = FakePort(throwOnStart = SecurityException("no RECORD_AUDIO"))
        val (core, port, pair) = setup(denied)
        core.start("Ava")
        pair.first.advance()
        assertTrue(pair.second.log.last(), pair.second.log.last().startsWith("error Platform(reason=Not permitted"))
        assertEquals(1, port.releases)

        val busy = FakePort(throwOnStart = IllegalStateException("busy"))
        val (core2, _, pair2) = setup(busy)
        core2.start("Ava")
        pair2.first.advance()
        assertTrue(pair2.second.log.last().startsWith("error Platform(reason=Recogniser call failed"))

        val french = FakePort(installed = listOf("fr-FR"))
        val (core3, port3, pair3) = setup(french)
        core3.start("Ava")
        pair3.first.advance()
        assertEquals("error LanguageNotInstalled(language=en-US, downloadable=true, pending=false)", pair3.second.log.last())
        assertEquals(0, port3.starts)

        val (core4, port4, pair4) = setup(FakePort(installed = listOf("en_US")))
        core4.start("Ava")
        pair4.first.advance()
        assertEquals(1, port4.starts)

        // All lists empty: unknown, not missing. Listening starts unverified.
        val (core6, port6, pair6) = setup(FakePort(installed = emptyList(), supported = emptyList()))
        core6.start("Ava")
        pair6.first.advance()
        assertEquals("Listening(languageVerified=false)", pair6.second.log.last())
        assertEquals(1, port6.starts)
        port6.events!!.onError(12) // the recogniser then says the language is missing
        assertEquals("error Recognizer(code=12)", pair6.second.log.last())

        val (core5, port5, pair5) = setup()
        core5.start("Ava!")
        assertEquals(listOf("error InvalidName"), pair5.second.log)
        assertEquals(0, port5.created.size)
    }

    @Test
    fun stopAndRestartReleaseEachSessionOnce() {
        val (core, port, pair) = setup()
        core.start("Ava")
        pair.first.advance()
        core.start("Ava") // restart
        pair.first.advance()
        core.stop()
        core.stop()
        assertEquals(2, port.releases)
        assertEquals(2, port.starts)
    }

    @Test
    fun restartsBackOffAndTerminalErrorsAreNotRetried() {
        val policy = WakeRestartPolicy()
        assertEquals(listOf(1_000L, 2_000L, 4_000L, 8_000L, 16_000L, 32_000L, 60_000L, 60_000L), (1..8).map { policy.nextDelayMs(0) })
        assertEquals(1_000L, policy.nextDelayMs(60_000)) // a stable session resets the backoff
        assertTrue(WakeRestartPolicy.retryable(WakeRecognizerError.Recognizer(11)))
        assertTrue(WakeRestartPolicy.retryable(WakeRecognizerError.Recognizer(10)))
        assertTrue(WakeRestartPolicy.retryable(WakeRecognizerError.Timeout("start")))
        for (terminal in listOf(
            WakeRecognizerError.Recognizer(9), WakeRecognizerError.Recognizer(12), WakeRecognizerError.Recognizer(13),
            WakeRecognizerError.Platform("x"), WakeRecognizerError.InvalidName,
            WakeRecognizerError.LanguageNotInstalled("en-US", true, false), WakeRecognizerError.Unsupported("x"),
        )) {
            assertFalse(terminal.toString(), WakeRestartPolicy.retryable(terminal))
        }
    }
}
