package dev.notdot.mobile

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.notdot_mobile_core.ContrastTake
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.personalModel
import uniffi.notdot_mobile_core.personalModelSummary
import uniffi.notdot_mobile_core.personalTake
import java.io.File
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.util.UUID
import kotlin.math.PI
import kotlin.math.sin

/**
 * The personal wake lab's sealed store on a real Android Keystore, with a
 * synthetic model built by the shipped core (generated tones; no microphone,
 * no recordings, no network). Uses its own folder and key alias, removed
 * afterwards, so the app's real personal-wake data is never touched.
 */
@RunWith(AndroidJUnit4::class)
class PersonalWakeStoreDeviceTest {
    private val target = InstrumentationRegistry.getInstrumentation().targetContext
    private val tag = UUID.randomUUID().toString()
    private val dir = File(target.cacheDir, "personal-wake-test-$tag")
    private val alias = "notdot-personal-wake-test-$tag"
    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    @After
    fun cleanUp() {
        dir.deleteRecursively()
        keystore().let { if (it.containsAlias(alias)) it.deleteEntry(alias) }
    }

    /** A tone "phrase" with silence around it, as 16 kHz 16-bit PCM. */
    private fun synthetic(pitches: List<Double>, gain: Double): ByteArray {
        val samples = ArrayList<Double>()
        repeat(4_800) { samples += 0.0 }
        for (p in pitches) {
            for (n in 0 until 3_200) {
                val env = sin(PI * n / 3_200)
                samples += gain * env * (sin(2 * PI * p * n / 16_000) + 0.5 * sin(2 * PI * 2.7 * p * n / 16_000))
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

    private fun syntheticModel(phrase: String): ByteArray {
        val takes = listOf(0.25, 0.3, 0.35, 0.4, 0.5).map { g ->
            personalTake(synthetic(listOf(300.0, 500.0, 400.0), g), 16_000u).also { assertNull(it.issue) }.features
        }
        val similar = personalTake(synthetic(listOf(300.0, 700.0, 400.0), 0.3), 16_000u).features
        return personalModel(phrase, takes, listOf(ContrastTake("Similar", similar))).model
    }

    @Test
    fun aSyntheticModelIsSealedReloadedReplacedAndDeleted() {
        val store = PersonalWakeStore(dir, KeystoreSealer(alias))
        assertNull(store.load())
        assertFalse(keystore().containsAlias(alias))

        val first = syntheticModel("Hey Ava")
        store.save(first)
        assertTrue(keystore().containsAlias(alias))
        assertEquals(listOf(PersonalWakeStore.NAME), dir.list()!!.toList())
        // Sealed at rest: the plaintext model header is not in the file.
        val onDisk = File(dir, PersonalWakeStore.NAME).readBytes()
        assertFalse(String(onDisk, Charsets.ISO_8859_1).contains("NDPW"))
        assertArrayEquals(first, store.load())
        assertEquals(5u, personalModelSummary(store.load()!!, "hey ava").phraseTakes)
        // Bound to its phrase: a renamed phrase is refused, never reused.
        assertThrows(MobileException::class.java) { personalModelSummary(store.load()!!, "Hey Nova") }

        // Re-enrolment replaces atomically; no partial file is left.
        val second = syntheticModel("Hey Nova")
        store.save(second)
        assertArrayEquals(second, store.load())
        assertEquals(listOf(PersonalWakeStore.NAME), dir.list()!!.toList())

        // Tampering is detected by the AES-GCM tag.
        val sealed = File(dir, PersonalWakeStore.NAME).readBytes()
        val flipped = sealed.copyOf().also { it[it.size - 5] = (it[it.size - 5].toInt() xor 1).toByte() }
        File(dir, PersonalWakeStore.NAME).writeBytes(flipped)
        assertThrows(GeneralSecurityException::class.java) { store.load() }
        File(dir, PersonalWakeStore.NAME).writeBytes(sealed)
        assertArrayEquals(second, store.load())

        // Delete removes the file, the folder and the key.
        store.delete()
        assertFalse(dir.exists())
        assertFalse(keystore().containsAlias(alias))
        assertNull(store.load())

        // A sealed file without its key cannot be read, and reading never
        // creates a key.
        dir.mkdirs()
        File(dir, PersonalWakeStore.NAME).writeBytes(sealed)
        assertThrows(GeneralSecurityException::class.java) { store.load() }
        assertFalse(keystore().containsAlias(alias))
    }

    @Test
    fun oversizedFilesAreRefusedBeforeDecrypting() {
        val store = PersonalWakeStore(dir, KeystoreSealer(alias))
        dir.mkdirs()
        File(dir, PersonalWakeStore.NAME).writeBytes(ByteArray((PersonalWakeStore.MAX_BYTES + 1).toInt()))
        assertThrows(GeneralSecurityException::class.java) { store.load() }
        assertFalse(keystore().containsAlias(alias))
    }
}
