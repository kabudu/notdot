package dev.notdot.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WakeSuppressionTest {
    private var now = 1_000L
    private val gate = PlaybackGate({ now })

    @Test
    fun wakeIsSuppressedDuringPlaybackAndTheHoldAfterIt() {
        assertFalse(gate.suppressed())
        assertFalse(gate.resumed())
        gate.playback(true)
        now += 5_000
        assertTrue(gate.suppressed())
        assertFalse(gate.resumed())
        gate.playback(false)
        now += PlaybackGate.HOLD_MS - 1
        assertTrue(gate.suppressed())
        assertFalse(gate.resumed())
        now += 1
        assertFalse(gate.suppressed())
        // The detector restarts exactly once after each reply.
        assertTrue(gate.resumed())
        assertFalse(gate.resumed())
    }

    @Test
    fun backToBackClipsKeepWakeSuppressed() {
        gate.playback(true)
        gate.playback(false)
        now += 100
        gate.playback(true)
        now += PlaybackGate.HOLD_MS * 3
        assertTrue(gate.suppressed())
        gate.playback(false)
        now += PlaybackGate.HOLD_MS
        assertFalse(gate.suppressed())
        assertTrue(gate.resumed())
    }

    @Test
    fun anEndWithoutAStartChangesNothing() {
        gate.playback(false)
        assertFalse(gate.suppressed())
        assertFalse(gate.resumed())
    }

    @Test
    fun savedPhrasesAreShownAsSpoken() {
        assertEquals("Hey Harriet", displayPhrase("hey harriet"))
        assertEquals("Ava", displayPhrase("ava"))
    }
}
