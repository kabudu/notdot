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
}
