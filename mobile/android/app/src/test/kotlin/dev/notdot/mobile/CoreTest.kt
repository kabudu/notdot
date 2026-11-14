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
import uniffi.notdot_mobile_core.BackupState
import uniffi.notdot_mobile_core.WakeEngine
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.defaultSessionPolicy
import uniffi.notdot_mobile_core.orbDefaultHue
import uniffi.notdot_mobile_core.orbFragmentGlsl
import uniffi.notdot_mobile_core.orbShaderSha256
import uniffi.notdot_mobile_core.orbUniformNames
import java.io.File
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertThrows
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

    /**
     * Feed `clip` after other speech, in 20 ms packets: faster than real time,
     * except while a backup decode runs (paced in real time, as live), then
     * silence while a decode is still running (up to 4 s).
     */
    private fun detects(engine: WakeEngine, clip: String, gain: Float): Boolean {
        val before = pcm16(File(repo, "src-tauri/tests/fixtures/negative.wav")).map { it * 0.3f }
        val wake = pcm16(File(repo, "src-tauri/tests/fixtures/$clip")).map { it * gain }
        val audio = before + List(3_200) { 0f } + wake + List(12_800) { 0f }
        engine.reset()
        for (chunk in audio.chunked(320)) {
            if (engine.backupBusy()) Thread.sleep(20)
            if (engine.accept(chunk, 16_000u)) return true
        }
        repeat(200) {
            if (!engine.backupBusy()) return false
            Thread.sleep(20)
            if (engine.accept(List(320) { 0f }, 16_000u)) return true
        }
        return false
    }

    private val models get() = File(repo, "src-tauri/resources/wake").absolutePath

    @Test
    fun wakeCorpusOfflineNeverFalseAccepts() {
        val engine = WakeEngine.spotterOnly(models, "Ava", false)
        assertEquals(BackupState.OFF, engine.backupState())
        val woke = listOf("hey-ava.wav", "hey-ava-uk.wav", "hey-ava-us.wav").filter { detects(engine, it, 0.35f) }
        println("spotter-only recall at 0.35: $woke")
        assertTrue(woke.isNotEmpty())
        for (clip in listOf("ava.wav", "negative.wav")) for (gain in listOf(0.05f, 0.35f, 0.8f)) {
            assertFalse("$clip@$gain", detects(engine, clip, gain))
        }
    }

    /** The full desktop detector: spotter plus the Whisper backup. */
    @Test
    fun fullDetectorWakesOnQuietSpeechAndNeverFalseAccepts() {
        val engine = WakeEngine(models, "Ava", false)
        val until = System.currentTimeMillis() + 60_000
        while (engine.backupState() == BackupState.LOADING && System.currentTimeMillis() < until) {
            engine.accept(emptyList(), 16_000u)
            Thread.sleep(20)
        }
        assertEquals(engine.backupError(), BackupState.READY, engine.backupState())
        for (gain in listOf(0.35f, 0.05f)) for (clip in listOf("hey-ava.wav", "hey-ava-uk.wav", "hey-ava-us.wav")) {
            assertTrue("$clip@$gain", detects(engine, clip, gain))
        }
        for (clip in listOf("ava.wav", "negative.wav")) for (gain in listOf(0.05f, 0.35f, 0.8f)) {
            assertFalse("$clip@$gain", detects(engine, clip, gain))
        }
        val stats = engine.backupStats()
        println("JVM backup stats: $stats")
        assertEquals(0uL, stats.failures)
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

    /** The adapter's public-key encoding, with a throwaway JVM key. */
    @Test
    fun stepUpKeyEncodingIsSec1() {
        assertArrayEquals(ByteArray(31) + byteArrayOf(5), StepUpKeys.fixed(BigInteger.valueOf(5)))
        val high = BigInteger.ONE.shiftLeft(255)
        assertEquals(32, StepUpKeys.fixed(high).size)
        val pair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
        val sec1 = StepUpKeys.sec1(pair.public as ECPublicKey)
        assertEquals(65, sec1.size)
        assertEquals(4.toByte(), sec1[0])
    }

    /** The sensitive-actions screen model against the debug-only scripted
     *  backend (a test adapter; never genuine hardware evidence). */
    @Test
    fun sensitiveActionsModelSetsUpErasesOnceAndReportsEveryRefusal() = kotlinx.coroutines.runBlocking {
        var unpaired: String? = null
        val backend = ScriptedSensitiveActions("setup")
        val model = SensitiveActionsModel(backend) { unpaired = it }
        model.refresh()
        assertFalse(model.canAct(model.status.value))
        assertTrue(model.statusText(model.status.value).startsWith("Not set up"))
        model.setUp()
        assertTrue(model.canAct(model.status.value))
        assertTrue(model.statusText(model.status.value).contains("test key, not real hardware"))
        model.eraseMemories()
        assertEquals("Erased 3 memories.", model.outcome.value?.text)
        assertEquals(1, backend.effects)
        assertNull(model.pendingResetId)

        for (scenario in listOf("cancelled", "denied", "expired", "offline", "revoked", "unavailable")) {
            val scripted = ScriptedSensitiveActions(scenario)
            val m = SensitiveActionsModel(scripted) { unpaired = it }
            m.refresh()
            if (scenario == "unavailable") m.setUp() else m.eraseMemories()
            val outcome = m.outcome.value
            assertTrue("$scenario: $outcome", outcome != null && outcome.problem)
            assertFalse(m.busy)
            assertEquals(scenario, 0, scripted.effects)
            assertEquals(scenario, scenario == "offline", m.pendingResetId != null)
            when (scenario) {
                "cancelled" -> assertEquals(SensitiveOutcome.Cancelled, outcome)
                "expired" -> assertEquals(SensitiveOutcome.Expired, outcome)
                "revoked" -> { assertTrue(outcome is SensitiveOutcome.Revoked); assertTrue(unpaired != null); assertNull(m.status.value) }
                "offline" -> assertTrue(outcome is SensitiveOutcome.Offline)
                "denied" -> assertTrue(outcome is SensitiveOutcome.Denied)
                "unavailable" -> assertTrue(outcome is SensitiveOutcome.Unavailable)
            }
        }
        // Not allowed at the computer: the action stays disabled.
        val notGranted = SensitiveActionsModel(ScriptedSensitiveActions("notgranted")) {}
        notGranted.refresh()
        assertFalse(notGranted.canAct(notGranted.status.value))
    }

}
