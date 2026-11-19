package dev.notdot.mobile

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.notdot_mobile_core.VoiceReplyListener
import uniffi.notdot_mobile_core.VoiceTurnResult
import java.util.Collections

/**
 * Tap to talk against the host Rust core (request endpointing), a fake
 * microphone and a fake computer. Synthetic tones only; nothing plays.
 */
class TapToTalkTest {
    private fun tone(ms: Int, amplitude: Double): ByteArray {
        val n = 16_000 * ms / 1000
        val out = ByteArray(n * 2)
        for (k in 0 until n) {
            val v = (Math.sin(k / 16_000.0 * 250.0 * 2 * Math.PI) * amplitude).toInt().toShort()
            out[2 * k] = (v.toInt() and 0xff).toByte()
            out[2 * k + 1] = (v.toInt() shr 8).toByte()
        }
        return out
    }

    /** Plays [script], then near-silence until closed (or fails at [failAt] bytes). */
    private inner class Mic(private val script: ByteArray, private val failAt: Int? = null) : PcmSource {
        @Volatile var closed = false
        @Volatile var bytesRead = 0
        private val quiet = tone(20, 20.0)
        override fun read(buffer: ByteArray): Int {
            if (closed) return -1
            if (failAt != null && bytesRead >= failAt) return -1
            Thread.sleep(2) // paced, so a tap can land mid-request
            val n = buffer.size
            for (i in 0 until n) buffer[i] = if (bytesRead + i < script.size) script[bytesRead + i] else quiet[(bytesRead + i) % quiet.size]
            bytesRead += n
            return n
        }
        override fun close() { closed = true }
    }

    private class Computer(val hold: CompletableDeferred<Unit>? = null) : VoiceTurnClient {
        val sent = Collections.synchronizedList(mutableListOf<ByteArray>())
        val cancelled = Collections.synchronizedList(mutableListOf<String>())
        override suspend fun ensureConnected() {}
        override suspend fun turn(requestId: String, pcm16: ByteArray, listener: VoiceReplyListener): VoiceTurnResult {
            sent.add(pcm16)
            listener.heard("what's running?")
            listener.replyText("Two tasks.")
            listener.replyClip(0u, "audio/mpeg", byteArrayOf(1))
            hold?.await()
            return VoiceTurnResult("what's running?", "Two tasks.", false)
        }
        override suspend fun cancel(requestId: String) { cancelled.add(requestId) }
    }

    private class Sink : ReplySink {
        val played = Collections.synchronizedList(mutableListOf<UInt>())
        @Volatile var stops = 0
        override fun play(clip: UInt, mime: String, audio: ByteArray) { played.add(clip) }
        override fun stopAll() { stops++ }
    }

    private class Rig(computer: Computer, val mic: () -> PcmSource?) {
        val sink = Sink()
        val states = Collections.synchronizedList(mutableListOf<TalkState>())
        val ui = Collections.synchronizedList(mutableListOf<VoiceUi>())
        var tap: TapToTalk? = null
        val turns = VoiceTurns(computer, sink, CoroutineScope(SupervisorJob() + Dispatchers.Default)) {
            ui.add(it)
            tap?.changed()
        }
        init {
            tap = TapToTalk(turns, mic, { false }, { states.add(it) })
        }
    }

    private suspend fun until(what: String, check: () -> Boolean) {
        try {
            withTimeout(5_000) { while (!check()) delay(5) }
        } catch (e: Exception) {
            throw AssertionError("timed out waiting for $what", e)
        }
    }

    private val request = tone(300, 20.0) + tone(1500, 6000.0) + tone(1000, 20.0)

    @Test
    fun aTapCapturesOneRequestSendsItAndClosesTheMicrophone() = runBlocking<Unit> {
        val computer = Computer()
        val mic = Mic(request)
        val rig = Rig(computer) { mic }
        val tap = rig.tap!!
        assertEquals(TalkState.IDLE, tap.state)
        assertTrue(tap.tap())
        assertEquals(TalkState.LISTENING, rig.states.first())
        until("the request to be sent") { computer.sent.size == 1 }
        until("the microphone to close") { mic.closed }
        until("the turn to finish") { rig.ui.toList().any { it is VoiceUi.Finished } && tap.state == TalkState.IDLE }
        val ms = computer.sent[0].size / 2 * 1000 / 16_000
        assertTrue("$ms ms", ms in 1500..2100)
        assertEquals(listOf(0u), rig.sink.played.toList())
        assertEquals(TalkState.IDLE, rig.states.last())
        // Listening stopped once the request ended, not after a fixed time.
        assertTrue("read ${mic.bytesRead} bytes", mic.bytesRead < (request.size + 32_000 * 2))
    }

    @Test
    fun aSecondTapWhileListeningStopsWithoutSending() = runBlocking<Unit> {
        val computer = Computer()
        val mic = Mic(tone(300, 20.0) + tone(5000, 6000.0))
        val rig = Rig(computer) { mic }
        rig.tap!!.tap()
        until("speech to be read") { mic.bytesRead > 16_000 }
        rig.tap!!.tap()
        until("the microphone to close") { mic.closed }
        delay(300)
        assertTrue(computer.sent.isEmpty())
        assertEquals(TalkState.IDLE, rig.tap!!.state)
    }

    @Test
    fun aTapWhileReplyingStopsTheReplyHereAndOnTheComputer() = runBlocking<Unit> {
        val hold = CompletableDeferred<Unit>()
        val computer = Computer(hold)
        val rig = Rig(computer) { Mic(request) }
        rig.tap!!.tap()
        until("the reply to start") { rig.sink.played.isNotEmpty() }
        assertEquals(TalkState.REPLYING, rig.tap!!.state)
        val stopsBefore = rig.sink.stops
        rig.tap!!.tap()
        until("the computer to be told") { computer.cancelled.size == 1 }
        assertTrue(rig.sink.stops > stopsBefore)
        assertEquals(TalkState.IDLE, rig.tap!!.state)
        hold.complete(Unit)
    }

    @Test
    fun microphoneFailuresEndTheTapCleanly() = runBlocking<Unit> {
        val computer = Computer()
        val none = Rig(computer) { null }
        assertFalse(none.tap!!.tap())
        assertEquals(TalkState.IDLE, none.tap!!.state)
        val mic = Mic(request, failAt = 8_000)
        val broken = Rig(computer) { mic }
        assertTrue(broken.tap!!.tap())
        until("the failed capture to end") { mic.closed && broken.tap!!.state == TalkState.IDLE }
        assertFalse(broken.turns.listening)
        assertTrue(computer.sent.isEmpty())
    }

    @Test
    fun hidingTheAppDropsAnUnfinishedRequestButKeepsAReply() = runBlocking<Unit> {
        val computer = Computer()
        val mic = Mic(tone(300, 20.0) + tone(5000, 6000.0))
        val rig = Rig(computer) { mic }
        rig.tap!!.tap()
        until("speech to be read") { mic.bytesRead > 16_000 }
        rig.tap!!.hidden()
        until("the microphone to close") { mic.closed }
        assertTrue(computer.sent.isEmpty())

        val hold = CompletableDeferred<Unit>()
        val slow = Computer(hold)
        val replying = Rig(slow) { Mic(request) }
        replying.tap!!.tap()
        until("the reply to start") { replying.sink.played.isNotEmpty() }
        val stops = replying.sink.stops
        replying.tap!!.hidden()
        assertEquals(stops, replying.sink.stops)
        assertTrue(slow.cancelled.isEmpty())
        hold.complete(Unit)
    }
}
