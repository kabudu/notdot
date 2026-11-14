package dev.notdot.mobile

import org.junit.Assert.*
import org.junit.Test

class PersonalWakeHistoryTest {
    @Test fun resultsSurviveReopeningInOrder() {
        val trials = listOf(WakeTrial(1, "Hey Ava", "Accepted"), WakeTrial(2, "Hey Ada", "Rejected"))
        assertEquals(trials, WakeTrialHistory.decode(WakeTrialHistory.encode(trials)))
    }
    @Test fun boundedJournalRetainsLatestHundred() {
        var trials = emptyList<WakeTrial>()
        repeat(107) { trials = WakeTrialHistory.append(trials, WakeTrial(it.toLong(), "phrase", "result")) }
        assertEquals(100, trials.size)
        assertEquals(7L, trials.first().time)
        assertEquals(106L, trials.last().time)
    }
    /** A journal written by the version 1 codec still decodes, as MFCC results. */
    @Test fun versionOneJournalsStillDecodeAsTheOriginalDetector() {
        val v1 = java.io.ByteArrayOutputStream().also { buffer ->
            java.io.DataOutputStream(buffer).use { out ->
                out.writeInt(1)
                out.writeInt(2)
                out.writeLong(1); out.writeUTF("Hey Ava"); out.writeUTF("Accepted")
                out.writeLong(2); out.writeUTF("Hey Ada"); out.writeUTF("Rejected")
            }
        }.toByteArray()
        val decoded = WakeTrialHistory.decode(v1)
        assertEquals(listOf(WakeTrial(1, "Hey Ava", "Accepted", "mfcc-v1"), WakeTrial(2, "Hey Ada", "Rejected", "mfcc-v1")), decoded)
        // Appending a phonetic result keeps both, each labelled.
        val mixed = WakeTrialHistory.append(decoded, WakeTrial(3, "Hey Eva", "Rejected", LabDetector.PHONETIC.id))
        assertEquals(listOf("mfcc-v1", "mfcc-v1", "phonetic-v1"), WakeTrialHistory.decode(WakeTrialHistory.encode(mixed)).map { it.detector })
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.encode(listOf(WakeTrial(1, "x", "y", ""))) }
    }

    /** Review regression: the largest valid journal (3-byte characters) round-trips, v1 and v2. */
    @Test fun maximalMultibyteJournalsRoundTrip() {
        val euro = '\u20AC' // three bytes in modified UTF-8
        val full = List(WakeTrialHistory.LIMIT) {
            WakeTrial(it.toLong(), euro.toString().repeat(120), euro.toString().repeat(1000), euro.toString().repeat(40))
        }
        val bytes = WakeTrialHistory.encode(full)
        assertTrue(bytes.size in 300_000..WakeTrialHistory.MAX_BYTES)
        assertEquals(full, WakeTrialHistory.decode(bytes))
        val v1 = java.io.ByteArrayOutputStream().also { buffer ->
            java.io.DataOutputStream(buffer).use { out ->
                out.writeInt(1)
                out.writeInt(WakeTrialHistory.LIMIT)
                full.forEach { out.writeLong(it.time); out.writeUTF(it.expected); out.writeUTF(it.outcome) }
            }
        }.toByteArray()
        val upgraded = WakeTrialHistory.decode(v1)
        assertEquals(full.map { it.copy(detector = "mfcc-v1") }, upgraded)
        assertEquals(upgraded, WakeTrialHistory.decode(WakeTrialHistory.encode(upgraded)))
        // Fits the sealed store (plaintext limit below its 1 MB file cap).
        assertTrue(WakeTrialHistory.MAX_BYTES <= PersonalWakeStore.MAX_BYTES - 64)
    }

    @Test fun detectorsKeepSeparateStoresAndTheOriginalKeepsItsLocation() {
        assertEquals("personal-wake", LabDetector.MFCC.folder)
        assertEquals("notdot-personal-wake", LabDetector.MFCC.alias)
        assertEquals(LabDetector.entries.size, LabDetector.entries.map { it.folder }.toSet().size)
        assertEquals(LabDetector.entries.size, LabDetector.entries.map { it.alias }.toSet().size)
        assertTrue(LabDetector.entries.none { it.folder == "wake" || it.folder == "personal-wake-history" })
    }

    /** A failed model load is reported as a setup problem, not "record again". */
    @Test fun takeProblemsNameTheDetectorAndKeepNothing() {
        val phonetic = takeProblem(LabDetector.PHONETIC, "The phonetic wake model is not the expected one")
        assertTrue(phonetic.contains("could not run") && phonetic.contains("not the expected one") && phonetic.contains("Nothing was kept"))
        assertFalse(phonetic.contains("Try again"))
        assertTrue(takeProblem(LabDetector.PHONETIC, null).contains("unknown error"))
        assertTrue(takeProblem(LabDetector.MFCC, "x".repeat(500)).length < 260)
        // The attempt still ends, so Record is offered again.
        val a = Attempts()
        val id = a.begin()
        assertTrue(a.finish(id) { })
        assertFalse(a.pending.value)
    }

    @Test fun corruptAndOversizedHistoryIsRefused() {
        assertThrows(Exception::class.java) { WakeTrialHistory.decode(byteArrayOf(1)) }
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.decode(ByteArray(WakeTrialHistory.MAX_BYTES + 1)) }
        val bytes = WakeTrialHistory.encode(emptyList())
        bytes[3] = 9
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.decode(bytes) }
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.encode(listOf(WakeTrial(1, "x".repeat(121), "result"))) }
    }
}
