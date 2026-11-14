package dev.notdot.mobile

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files

/** JVM tests of the personal wake lab's framework-free parts. */
class PersonalWakeLabTest {
    private fun packet(level: Short): ByteArray {
        val p = ByteArray(640)
        for (i in 0 until 320) {
            val v = if (i % 2 == 0) level else (-level).toShort()
            p[2 * i] = (v.toInt() and 0xff).toByte()
            p[2 * i + 1] = (v.toInt() shr 8).toByte()
        }
        return p
    }

    private fun PhraseEndpointer.feed(level: Short, count: Int): PhraseEndpointer.State {
        var s = state
        repeat(count) {
            val p = packet(level)
            s = push(p, PhraseEndpointer.rms(p))
        }
        return s
    }

    @Test
    fun theEndpointerCapturesOnePhraseWithPreRoll() {
        val e = PhraseEndpointer(PhraseEndpointer.TAKE_MAX_MS)
        assertEquals(PhraseEndpointer.State.WAITING, e.feed(30, 25)) // 0.5 s room floor
        assertEquals(PhraseEndpointer.State.SPEAKING, e.feed(6_000, 30)) // 0.6 s of speech
        assertEquals(0, e.audio().size)
        assertEquals(PhraseEndpointer.State.SPEAKING, e.feed(30, 30)) // 0.6 s quiet: not yet
        assertEquals(PhraseEndpointer.State.DONE, e.feed(30, 10))
        // 300 ms pre-roll + speech + 700 ms of trailing quiet.
        assertEquals(PhraseEndpointer.PRE_ROLL_BYTES + 640 * 29 + 640 * 35, e.audio().size)
        assertEquals(PhraseEndpointer.State.DONE, e.feed(6_000, 5)) // ignored once done
    }

    @Test
    fun theEndpointerGivesUpOnSilenceAndCapsLength() {
        val quiet = PhraseEndpointer(PhraseEndpointer.TAKE_MAX_MS)
        assertEquals(PhraseEndpointer.State.NO_SPEECH, quiet.feed(30, 250))
        assertEquals(0, quiet.audio().size)
        val long = PhraseEndpointer(1_000)
        long.feed(30, 20)
        assertEquals(PhraseEndpointer.State.DONE, long.feed(6_000, 100))
        assertTrue(long.audio().size <= PhraseEndpointer.PRE_ROLL_BYTES + 32 * 1_000)
        // A loud room raises the bar: hum at 4x the floor is not speech.
        val loud = PhraseEndpointer(PhraseEndpointer.TAKE_MAX_MS)
        loud.feed(1_000, 20)
        assertEquals(PhraseEndpointer.State.WAITING, loud.feed(3_000, 20))
    }

    @Test
    fun enrolmentNeedsFivePhraseTakesAndSimilarOnesAreOptional() {
        val e = PersonalEnrolment("Hey Ava", listOf("Hey Ada", " hey ada ", "Hey Eva", "Hey Nova", "Hey Bob", "hey ava"))
        assertEquals(listOf("Hey Ada", "Hey Eva", "Hey Nova"), e.similarLabels)
        repeat(5) { i ->
            assertEquals(PersonalEnrolment.Target.Phrase(i), e.next())
            assertFalse(e.canSave)
            e.accept(e.next(), byteArrayOf(i.toByte()))
        }
        assertTrue(e.canSave)
        assertEquals(PersonalEnrolment.Target.Similar("Hey Ada"), e.next())
        e.accept(e.next(), byteArrayOf(9))
        e.skipRemainingSimilar()
        assertEquals(PersonalEnrolment.Target.Review, e.next())
        assertEquals(listOf("Hey Ada"), e.similarFeatures().map { it.first })
        e.redo(PersonalEnrolment.Target.Phrase(2))
        assertFalse(e.canSave)
        assertEquals(PersonalEnrolment.Target.Phrase(2), e.next())
        e.accept(e.next(), byteArrayOf(7))
        e.redo(PersonalEnrolment.Target.Similar("Hey Eva"))
        assertEquals(PersonalEnrolment.Target.Similar("Hey Eva"), e.next())
        e.clear()
        assertEquals(0, e.phraseCount)
        assertThrows(IllegalArgumentException::class.java) { PersonalEnrolment(" ", emptyList()) }
    }

    /** XOR "sealing" with the name as a check prefix: enough to test the store. */
    private class FakeSealer : Sealer {
        var destroyed = 0
        override fun seal(name: String, data: ByteArray) = name.toByteArray() + data.map { (it.toInt() xor 0x5a).toByte() }
        override fun open(name: String, sealed: ByteArray): ByteArray {
            val prefix = name.toByteArray()
            if (sealed.size < prefix.size || !sealed.copyOf(prefix.size).contentEquals(prefix)) {
                throw java.security.GeneralSecurityException("tag mismatch")
            }
            return sealed.copyOfRange(prefix.size, sealed.size).map { (it.toInt() xor 0x5a).toByte() }.toByteArray()
        }
        override fun destroyKey() {
            destroyed++
        }
    }

    /** Runs submitted work and main-thread deliveries only when told to. */
    private class ManualWorker : SerialWorker {
        val work = ArrayDeque<() -> Unit>()
        val main = ArrayDeque<() -> Unit>()
        override fun submit(work: () -> Unit) {
            this.work.addLast(work)
        }
        override fun main(action: () -> Unit) {
            main.addLast(action)
        }
        fun runWork() {
            while (work.isNotEmpty()) work.removeFirst()()
        }
        fun runMain() {
            while (main.isNotEmpty()) main.removeFirst()()
        }
    }

    private fun lab(block: (File, PersonalWakeStore, FakeSealer) -> Unit) {
        val root = Files.createTempDirectory("notdot-personal").toFile()
        try {
            val sealer = FakeSealer()
            block(root, PersonalWakeStore(File(root, PersonalWakeStore.FOLDER), sealer), sealer)
        } finally {
            root.setWritable(true)
            File(root, PersonalWakeStore.FOLDER).setWritable(true)
            root.deleteRecursively()
        }
    }

    @Test
    fun aTakeFromACancelledEnrolmentNeverReachesTheNextOne() = lab { _, store, _ ->
        val worker = ManualWorker()
        val c = PersonalLabCoordinator(worker, store)
        val first = c.invalidate()
        val delivered = mutableListOf<String>()
        c.compute(first, { "old take" }) { delivered += it }
        worker.runWork()
        val second = c.invalidate() // cancel, then a new enrolment
        worker.runMain()
        c.compute(second, { "new take" }) { delivered += it }
        worker.runWork()
        worker.runMain()
        assertEquals(listOf("new take"), delivered)
    }

    @Test
    fun aCancelDuringASaveStopsTheWriteAndKeepsTheOldModel() = lab { _, store, _ ->
        store.save(byteArrayOf(1))
        val worker = ManualWorker()
        val c = PersonalLabCoordinator(worker, store)
        val id = c.invalidate()
        var outcome: Result<Unit>? = null
        assertTrue(c.save(id, { byteArrayOf(2) }) { outcome = it })
        assertTrue(c.busy)
        assertFalse("one save at a time", c.save(id, { byteArrayOf(3) }) {})
        c.invalidate() // Cancel tapped while the model was still being built
        worker.runWork()
        worker.runMain()
        assertFalse(c.busy)
        assertNull("a stale save reports nothing", outcome)
        assertArrayEquals(byteArrayOf(1), store.load())
        // A current save does write and report.
        val next = c.current()
        assertTrue(c.save(next, { byteArrayOf(4) }) { outcome = it })
        worker.runWork()
        worker.runMain()
        assertTrue(outcome!!.isSuccess)
        assertArrayEquals(byteArrayOf(4), store.load())
    }

    @Test
    fun aSaveQueuedBeforeADeleteNeverRecreatesTheModel() = lab { root, store, sealer ->
        store.save(byteArrayOf(1))
        val worker = ManualWorker()
        val c = PersonalLabCoordinator(worker, store)
        val id = c.invalidate()
        c.save(id, { byteArrayOf(2) }) {}
        var deleted: Result<Unit>? = null
        c.delete { deleted = it }
        worker.runWork()
        worker.runMain()
        assertTrue(deleted!!.isSuccess)
        assertNull(store.load())
        assertFalse(File(root, PersonalWakeStore.FOLDER).exists())
        assertEquals(1, sealer.destroyed)
    }

    @Test
    fun deleteFailuresAreReportedNotHidden() = lab { root, store, sealer ->
        store.save(byteArrayOf(1))
        val dir = File(root, PersonalWakeStore.FOLDER)
        assertTrue(dir.setWritable(false))
        assertThrows(java.io.IOException::class.java) { store.delete() }
        assertEquals("the key is kept while the file remains", 0, sealer.destroyed)
        assertTrue(dir.setWritable(true))
        val keyFails = PersonalWakeStore(dir, object : Sealer by sealer {
            override fun destroyKey() = throw java.security.GeneralSecurityException("keystore busy")
        })
        assertThrows(java.security.GeneralSecurityException::class.java) { keyFails.delete() }
        // Oversized models are refused before sealing.
        assertThrows(IllegalArgumentException::class.java) { store.save(ByteArray(PersonalWakeStore.MAX_BYTES.toInt())) }
    }

    @Test
    fun theStoreReplacesAtomicallyRefusesTamperingAndDeletesEverything() {
        val root = Files.createTempDirectory("notdot-personal").toFile()
        try {
            val wake = File(root, "wake").apply { mkdirs() }
            File(wake, "123").mkdirs()
            val dir = File(root, PersonalWakeStore.FOLDER)
            val sealer = FakeSealer()
            val store = PersonalWakeStore(dir, sealer)
            assertNull(store.load())
            store.save(byteArrayOf(1, 2, 3))
            assertArrayEquals(byteArrayOf(1, 2, 3), store.load())
            store.save(byteArrayOf(4, 5))
            assertArrayEquals(byteArrayOf(4, 5), store.load())
            assertEquals(listOf(PersonalWakeStore.NAME), dir.list()!!.toList())
            // The generated model folder is a sibling, never a parent.
            assertEquals(root, dir.parentFile)
            assertTrue(File(wake, "123").exists())
            // A failed write leaves the previous model in place.
            val failing = PersonalWakeStore(dir, object : Sealer by sealer {
                override fun seal(name: String, data: ByteArray): ByteArray = throw IllegalStateException("keystore")
            })
            assertThrows(IllegalStateException::class.java) { failing.save(byteArrayOf(9)) }
            assertArrayEquals(byteArrayOf(4, 5), store.load())
            // An oversized file is refused before it is read.
            File(dir, PersonalWakeStore.NAME).writeBytes(ByteArray((PersonalWakeStore.MAX_BYTES + 1).toInt()))
            assertThrows(java.security.GeneralSecurityException::class.java) { store.load() }
            // Tampering is detected, not silently accepted.
            File(dir, PersonalWakeStore.NAME).writeBytes(byteArrayOf(1, 2, 3))
            assertThrows(java.security.GeneralSecurityException::class.java) { store.load() }
            store.delete()
            assertFalse(dir.exists())
            assertNull(store.load())
            assertEquals(1, sealer.destroyed)
            assertTrue(File(wake, "123").exists())
        } finally {
            root.deleteRecursively()
        }
    }
}
