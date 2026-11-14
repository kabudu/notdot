package dev.notdot.mobile

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.phoneticAvailable
import uniffi.notdot_mobile_core.phoneticMatch
import uniffi.notdot_mobile_core.phoneticModel
import uniffi.notdot_mobile_core.phoneticModelSummary
import uniffi.notdot_mobile_core.phoneticTake
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.io.File
import java.security.KeyStore
import java.util.UUID
import kotlin.math.PI
import kotlin.math.sin

/**
 * Silent smoke tests of the phonetic lab candidate on a device: generated
 * tones only (no microphone, no playback, no real enrolment or history),
 * with test-only folders and key aliases that are removed afterwards. Needs
 * an app built with NOTDOT_PERSONAL_PHONETIC=1 and -PnotdotPersonalWakeLab=true.
 */
@RunWith(AndroidJUnit4::class)
class PersonalPhoneticDeviceTest {
    private val target = InstrumentationRegistry.getInstrumentation().targetContext
    private val tag = UUID.randomUUID().toString()
    private val root = File(target.cacheDir, "personal-phonetic-test-$tag")
    private val aliases = mutableListOf<String>()
    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    @After
    fun cleanUp() {
        root.deleteRecursively()
        val ks = keystore()
        aliases.forEach { if (ks.containsAlias(it)) ks.deleteEntry(it) }
    }

    private fun alias(name: String) = "$name-test-$tag".also { aliases += it }

    /** A tone "phrase" with silence around it, as 16 kHz 16-bit PCM. */
    private fun tones(pitches: List<Double>, gain: Double): ByteArray {
        val samples = ArrayList<Double>()
        repeat(4_800) { samples += 0.0 }
        for (p in pitches) {
            for (n in 0 until 3_200) {
                val env = sin(PI * n / 3_200)
                samples += gain * env * sin(2 * PI * p * n / 16_000)
            }
        }
        repeat(4_800) { samples += 0.0 }
        var noise = 7
        val out = ByteArray(samples.size * 2)
        samples.forEachIndexed { i, v ->
            noise = noise xor (noise shl 13); noise = noise xor (noise ushr 17); noise = noise xor (noise shl 5)
            val s = ((v + (noise.toDouble() / Int.MAX_VALUE) * 1e-4).coerceIn(-1.0, 1.0) * 32767).toInt()
            out[2 * i] = (s and 0xff).toByte()
            out[2 * i + 1] = (s shr 8).toByte()
        }
        return out
    }

    @Test
    fun selectionReopensAndGuidedPromptsLabelAllSteps() {
        val name = "guided-selection-test-$tag"
        val preferences = target.getSharedPreferences(name, android.content.Context.MODE_PRIVATE)
        try {
            assertTrue(preferences.edit().putString("detector", LabDetector.PHONETIC.id).commit())
            val reopened = target.getSharedPreferences(name, android.content.Context.MODE_PRIVATE)
            assertEquals(LabDetector.PHONETIC, selectedLabDetector(reopened.getString("detector", null), true))
            assertTrue(preferences.edit().putString("detector", LabDetector.MFCC.id).commit())
            assertEquals(LabDetector.MFCC, selectedLabDetector(reopened.getString("detector", null), true))
            val run = GuidedWakeCheck("Hey Ava", listOf("Hey Ada", "Hey Eva", "Hey Nova"))
            assertEquals(14, run.prompts.size)
            run.prompts.forEachIndexed { step, prompt ->
                assertEquals(prompt, run.current)
                run.saved(step)
            }
            assertTrue(run.complete)
        } finally {
            target.deleteSharedPreferences(name)
        }
    }

    @Test
    fun theCandidateIsBuiltInAndItsPinnedModelLoads() {
        assertTrue("build the core with NOTDOT_PERSONAL_PHONETIC=1", phoneticAvailable())
        val models = WakeModels.directory(target)
        for (f in listOf("encoder.onnx", "decoder.onnx", "joiner.onnx")) {
            assertTrue("$f installed", File(models, "phonetic/$f").isFile)
        }
        // A take loads the pinned model (hash and schema checked) and runs it.
        val take = phoneticTake(models.absolutePath, tones(listOf(300.0, 500.0, 400.0), 0.3), 16_000u)
        assertNull(take.issue)
        assertTrue(take.features.isNotEmpty())
        // A folder without the model is a clear error, not a crash.
        val missing = File(root, "no-models").apply { mkdirs() }
        assertThrows(MobileException::class.java) {
            phoneticTake(missing.absolutePath, tones(listOf(300.0, 500.0, 400.0), 0.3), 16_000u)
        }
    }

    @Test
    fun takeModelMatchAndSummaryRunEndToEnd() {
        val models = WakeModels.directory(target).absolutePath
        val takes = listOf(0.25, 0.3, 0.35, 0.4, 0.5).map { g ->
            phoneticTake(models, tones(listOf(300.0, 500.0, 400.0), g), 16_000u).also { assertNull(it.issue) }.features
        }
        val model = phoneticModel("Hey Ava", takes, emptyList()).model
        val summary = phoneticModelSummary(model, "hey ava")
        assertEquals(5u, summary.phraseTakes)
        val same = phoneticMatch(models, model, "Hey Ava", tones(listOf(300.0, 500.0, 400.0), 0.45), 16_000u)
        val other = phoneticMatch(models, model, "Hey Ava", tones(listOf(900.0, 150.0), 0.3), 16_000u)
        assertNotNull(same.nearestPhrase)
        assertTrue("${same.nearestPhrase} < ${other.nearestPhrase}", same.nearestPhrase!! < other.nearestPhrase!!)
        val silent = phoneticMatch(models, model, "Hey Ava", ByteArray(32_000), 16_000u)
        assertNotNull(silent.issue)
        assertFalse(silent.matched)
        // Bound to its phrase and format: renamed or damaged models are refused.
        assertThrows(MobileException::class.java) { phoneticModelSummary(model, "Hey Nova") }
        assertThrows(MobileException::class.java) { phoneticModelSummary(model.copyOf(model.size - 1), "Hey Ava") }
    }

    @Test
    fun detectorStoresAreSeparateAndV1HistoryStillOpens() {
        val mfcc = PersonalWakeStore(File(root, LabDetector.MFCC.folder), KeystoreSealer(alias(LabDetector.MFCC.alias)), LabDetector.MFCC.maxBytes)
        val phonetic = PersonalWakeStore(File(root, LabDetector.PHONETIC.folder), KeystoreSealer(alias(LabDetector.PHONETIC.alias)), LabDetector.PHONETIC.maxBytes)
        mfcc.save(byteArrayOf(1, 2, 3))
        val before = File(root, "${LabDetector.MFCC.folder}/${PersonalWakeStore.NAME}").readBytes()
        phonetic.save(ByteArray(2_000_000) { (it % 251).toByte() }) // larger than the MFCC store allows
        phonetic.delete()
        assertArrayEquals(before, File(root, "${LabDetector.MFCC.folder}/${PersonalWakeStore.NAME}").readBytes())
        assertArrayEquals(byteArrayOf(1, 2, 3), mfcc.load())
        assertNull(phonetic.load())

        // A version 1 journal, sealed and reopened, decodes as the original detector's.
        val v1 = ByteArrayOutputStream().also { buffer ->
            DataOutputStream(buffer).use { out ->
                out.writeInt(1); out.writeInt(1)
                out.writeLong(1); out.writeUTF("Hey Ava"); out.writeUTF("Accepted")
            }
        }.toByteArray()
        val history = PersonalWakeStore(File(root, "personal-wake-history"), KeystoreSealer(alias("notdot-personal-wake-history")))
        history.save(v1)
        val trials = WakeTrialHistory.decode(history.load()!!)
        assertEquals(listOf(WakeTrial(1, "Hey Ava", "Accepted", "mfcc-v1")), trials)
        val next = WakeTrialHistory.append(trials, WakeTrial(2, "Hey Ada", "Rejected", LabDetector.PHONETIC.id))
        history.save(WakeTrialHistory.encode(next))
        assertEquals(listOf("mfcc-v1", "phonetic-v1"), WakeTrialHistory.decode(history.load()!!).map { it.detector })
    }
}
