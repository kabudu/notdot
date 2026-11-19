package dev.notdot.mobile

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.VoiceReplyListener
import uniffi.notdot_mobile_core.VoiceTurnResult
import java.util.Collections

/**
 * The Android voice-turn controller against the host Rust core (request
 * endpointing) and a fake computer. Synthetic tones only; nothing plays.
 */
class VoiceTurnsTest {
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

    private class FakeClient(val hold: CompletableDeferred<Unit>? = null) : VoiceTurnClient {
        val sent = Collections.synchronizedList(mutableListOf<ByteArray>())
        val cancelled = Collections.synchronizedList(mutableListOf<String>())
        val done = CompletableDeferred<Unit>()
        override suspend fun ensureConnected() {}
        override suspend fun turn(requestId: String, pcm16: ByteArray, listener: VoiceReplyListener): VoiceTurnResult {
            sent.add(pcm16)
            listener.heard("what's still running?")
            listener.replyText("Two tasks are running.")
            listener.replyClip(0u, "audio/mpeg", byteArrayOf(1, 2))
            hold?.await()
            listener.replyText("The build is nearly done.")
            listener.replyClip(1u, "audio/mpeg", byteArrayOf(3))
            done.complete(Unit)
            return VoiceTurnResult("what's still running?", "Two tasks are running. The build is nearly done.", false)
        }
        override suspend fun cancel(requestId: String) {
            cancelled.add(requestId)
        }
    }

    private class Sink : ReplySink {
        val played = Collections.synchronizedList(mutableListOf<UInt>())
        var stops = 0
        override fun play(clip: UInt, mime: String, audio: ByteArray) { played.add(clip) }
        override fun stopAll() { stops++ }
    }

    private fun feed(turns: VoiceTurns, audio: ByteArray) {
        audio.toList().chunked(640).forEach { turns.feed(it.toByteArray()) }
    }

    @Test
    fun aRequestAfterTheWakePhraseIsSentAndItsReplyPlaysInOrder() = runBlocking {
        val client = FakeClient()
        val sink = Sink()
        val seen = Collections.synchronizedList(mutableListOf<VoiceUi>())
        val turns = VoiceTurns(client, sink, CoroutineScope(SupervisorJob() + Dispatchers.Default)) { seen.add(it) }
        feed(turns, tone(500, 6000.0)) // Ignored: no wake yet.
        turns.wake()
        feed(turns, tone(300, 20.0) + tone(1500, 6000.0) + tone(1000, 20.0))
        withTimeout(5_000) { client.done.await() }
        withTimeout(5_000) { while (seen.toList().none { it is VoiceUi.Finished }) kotlinx.coroutines.delay(10) }
        assertEquals(1, client.sent.size)
        val ms = client.sent[0].size / 2 * 1000 / 16_000
        assertTrue("$ms ms", ms in 1500..2100)
        assertEquals(listOf(0u, 1u), sink.played.toList())
        assertEquals(VoiceUi.Listening, seen.first())
        assertTrue(seen.contains(VoiceUi.Heard("what's still running?")))
        assertTrue(!turns.listening)
    }

    @Test
    fun stopCancelsTheTurnOnTheComputerAndPlaysNothingMore() = runBlocking {
        val hold = CompletableDeferred<Unit>()
        val client = FakeClient(hold)
        val sink = Sink()
        val turns = VoiceTurns(client, sink, CoroutineScope(SupervisorJob() + Dispatchers.Default)) {}
        turns.wake()
        feed(turns, tone(1500, 6000.0) + tone(1000, 20.0))
        withTimeout(5_000) { while (sink.played.isEmpty()) kotlinx.coroutines.delay(10) }
        turns.stop()
        hold.complete(Unit)
        withTimeout(5_000) { while (client.cancelled.isEmpty()) kotlinx.coroutines.delay(10) }
        withTimeout(5_000) { client.done.await() }
        assertEquals(1, client.cancelled.size)
        assertEquals("a cancelled turn's later clips never play", listOf(0u), sink.played.toList())
        assertTrue(sink.stops >= 1)
    }

    @Test
    fun silenceAfterTheWakePhraseSendsNothingAndFailuresArePlain() = runBlocking {
        val client = FakeClient()
        val seen = Collections.synchronizedList(mutableListOf<VoiceUi>())
        val turns = VoiceTurns(client, Sink(), CoroutineScope(SupervisorJob() + Dispatchers.Default)) { seen.add(it) }
        turns.wake()
        feed(turns, tone(6_500, 20.0))
        assertTrue(client.sent.isEmpty())
        assertTrue(seen.last() is VoiceUi.NothingHeard)

        val offline = object : VoiceTurnClient {
            override suspend fun ensureConnected() = throw MobileException.Offline("Not connected to the computer.")
            override suspend fun turn(requestId: String, pcm16: ByteArray, listener: VoiceReplyListener) = error("unreachable")
            override suspend fun cancel(requestId: String) {}
        }
        val failures = Collections.synchronizedList(mutableListOf<VoiceUi>())
        val again = VoiceTurns(offline, Sink(), CoroutineScope(SupervisorJob() + Dispatchers.Default)) { failures.add(it) }
        again.wake()
        feed(again, tone(1500, 6000.0) + tone(1000, 20.0))
        withTimeout(5_000) { while (failures.toList().none { it is VoiceUi.Failed }) kotlinx.coroutines.delay(10) }
        assertEquals(VoiceUi.Failed("Not connected to the computer."), failures.last())
    }
}
