package dev.notdot.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.notdot_mobile_core.OrbEngine
import uniffi.notdot_mobile_core.OrbMode
import uniffi.notdot_mobile_core.OrbQuota
import uniffi.notdot_mobile_core.OrbState
import uniffi.notdot_mobile_core.OrbTask
import uniffi.notdot_mobile_core.PauseReason
import uniffi.notdot_mobile_core.Platform
import uniffi.notdot_mobile_core.SessionEvent
import uniffi.notdot_mobile_core.SessionState
import uniffi.notdot_mobile_core.StartSource
import uniffi.notdot_mobile_core.VoiceSession
import uniffi.notdot_mobile_core.WakeEngine
import uniffi.notdot_mobile_core.defaultSessionPolicy
import uniffi.notdot_mobile_core.orbDefaultHue
import uniffi.notdot_mobile_core.orbFragmentGlsl
import uniffi.notdot_mobile_core.orbShaderSha256
import uniffi.notdot_mobile_core.orbUniformNames
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** JVM tests of the Kotlin adapter boundary, against the host Rust build. */
class CoreTest {
    private val repo = File(System.getProperty("notdot.repo")!!)

    @Test
    fun androidShaderIsTheCanonicalSourceVerbatim() {
        val canonical = File(repo, "crates/notdot-orb/src/orb.frag").readText()
        val glsl = orbFragmentGlsl()
        assertTrue(glsl.endsWith(canonical))
        assertTrue(glsl.contains("orb.frag sha256=${orbShaderSha256()}"))
        assertEquals(
            listOf("resolution", "tint", "phase", "energy", "listening", "thinking", "speaking", "still", "task_kind", "task_time", "task_strength"),
            orbUniformNames(),
        )
    }

    @Test
    fun orbFramesFollowTheDesktopClock() {
        val engine = OrbEngine()
        val state = OrbState(OrbMode.SPEAKING, OrbTask.NONE, orbDefaultHue(), null, false, OrbQuota.NORMAL, false, true, false, false)
        val frame = engine.frame(state, 0.0, 300f, 300f)!!
        assertEquals(14, frame.packed.size)
        assertNull(engine.frame(state, 16.0, 300f, 300f))
        assertEquals("Speaking. Microphone muted.", frame.accessibilityLabel)
    }

    @Test
    fun handsFreeSessionThroughTheSharedCore() {
        val session = VoiceSession(defaultSessionPolicy(Platform.ANDROID).copy(microphonePermitted = true))
        session.handle(SessionEvent.AppVisible, 0u)
        val started = session.handle(SessionEvent.Start(StartSource.APP), 0u)
        assertEquals(SessionState.HandsFree, started.state)
        assertEquals(600_000uL, started.remainingMs)
        assertTrue(session.handle(SessionEvent.AppHidden, 1_000u).microphone)
        val silenced = session.handle(SessionEvent.Silenced(true), 2_000u)
        assertEquals(SessionState.Paused(PauseReason.SILENCED), silenced.state)
        assertFalse(silenced.microphone)
        // Android sessions have no eight-hour surface limit (iOS does).
        session.handle(SessionEvent.Silenced(false), 3_000u)
        var now = 3_000uL
        while (now < 9uL * 3_600_000uL) {
            now += 300_000uL
            session.handle(SessionEvent.Activity, now)
        }
        assertEquals(SessionState.HandsFree, session.state())
    }

    @Test
    fun wakeCorpusOfflineNeverFalseAccepts() {
        val engine = WakeEngine(File(repo, "src-tauri/resources/wake").absolutePath, "Ava", false)
        fun detects(clip: String, gain: Float): Boolean {
            val before = pcm16(File(repo, "src-tauri/tests/fixtures/negative.wav")).map { it * 0.3f }
            val wake = pcm16(File(repo, "src-tauri/tests/fixtures/$clip")).map { it * gain }
            val audio = before + List(3_200) { 0f } + wake + List(12_800) { 0f }
            engine.reset()
            return audio.chunked(320).fold(false) { hit, chunk -> engine.accept(chunk, 16_000u) || hit }
        }
        val woke = listOf("hey-ava.wav", "hey-ava-uk.wav", "hey-ava-us.wav").filter { detects(it, 0.35f) }
        println("spotter-only recall at 0.35: $woke")
        assertTrue(woke.isNotEmpty())
        for (clip in listOf("ava.wav", "negative.wav")) for (gain in listOf(0.05f, 0.35f, 0.8f)) {
            assertFalse("$clip@$gain", detects(clip, gain))
        }
    }

    /** 16 kHz mono 16-bit PCM WAV, data chunk only. */
    private fun pcm16(file: File): List<Float> {
        val bytes = file.readBytes()
        var offset = 12
        while (offset + 8 <= bytes.size) {
            val id = String(bytes, offset, 4)
            val size = ByteBuffer.wrap(bytes, offset + 4, 4).order(ByteOrder.LITTLE_ENDIAN).int
            if (id == "data") {
                val data = ByteBuffer.wrap(bytes, offset + 8, size).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
                return List(data.remaining()) { data.get(it) / 32768f }
            }
            offset += 8 + size + (size and 1)
        }
        error("no data chunk in $file")
    }
}
