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
    @Test fun corruptAndOversizedHistoryIsRefused() {
        assertThrows(Exception::class.java) { WakeTrialHistory.decode(byteArrayOf(1)) }
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.decode(ByteArray(256_001)) }
        val bytes = WakeTrialHistory.encode(emptyList())
        bytes[3] = 2
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.decode(bytes) }
        assertThrows(IllegalArgumentException::class.java) { WakeTrialHistory.encode(listOf(WakeTrial(1, "x".repeat(121), "result"))) }
    }
}
