package dev.notdot.mobile

import android.opengl.EGL14
import android.opengl.GLES20
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.notdot_mobile_core.BackupState
import uniffi.notdot_mobile_core.LinkRoute
import uniffi.notdot_mobile_core.SessionEvent
import uniffi.notdot_mobile_core.StartSource
import uniffi.notdot_mobile_core.PairResult
import uniffi.notdot_mobile_core.AttestRequest
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.StepUpEvidence
import uniffi.notdot_mobile_core.newRequestId
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.zip.Inflater
import kotlin.math.abs

@RunWith(AndroidJUnit4::class)
class DeviceTest {
    private val context = InstrumentationRegistry.getInstrumentation().context

    private fun asset(path: String) = context.assets.open(path).use { it.readBytes() }

    private fun inflate(bytes: ByteArray, size: Int): ByteArray {
        val inflater = Inflater(true)
        inflater.setInput(bytes)
        val out = ByteArray(size)
        var read = 0
        while (read < size && !inflater.finished()) read += inflater.inflate(out, read, size - read)
        inflater.end()
        return out
    }

    /**
     * Render every golden frame with the canonical GLSL on this device's
     * GLES driver (offscreen pbuffer) and compare with the goldens.
     */
    @Test
    fun goldenFramesMatchOnThisGpu() {
        val index = JSONObject(String(asset("orb-golden/index.json")))
        val size = index.getInt("size")
        val display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY)
        EGL14.eglInitialize(display, IntArray(2), 0, IntArray(2), 1)
        val configs = arrayOfNulls<android.opengl.EGLConfig>(1)
        val count = IntArray(1)
        EGL14.eglChooseConfig(
            display,
            intArrayOf(EGL14.EGL_RENDERABLE_TYPE, EGL14.EGL_OPENGL_ES2_BIT, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_PBUFFER_BIT,
                EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8, EGL14.EGL_ALPHA_SIZE, 8, EGL14.EGL_NONE),
            0, configs, 0, 1, count, 0,
        )
        val context = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT, intArrayOf(EGL14.EGL_CONTEXT_CLIENT_VERSION, 2, EGL14.EGL_NONE), 0)
        val surface = EGL14.eglCreatePbufferSurface(display, configs[0], intArrayOf(EGL14.EGL_WIDTH, size, EGL14.EGL_HEIGHT, size, EGL14.EGL_NONE), 0)
        assertTrue(EGL14.eglMakeCurrent(display, surface, surface, context))
        try {
            val orb = OrbGl()
            val frames = index.getJSONArray("frames")
            var worst = 0.0
            var worstName = ""
            for (i in 0 until frames.length()) {
                val frame = frames.getJSONObject(i)
                val packedJson = frame.getJSONArray("packed")
                val packed = List(packedJson.length()) { packedJson.getDouble(it).toFloat() }.toMutableList()
                packed[0] = size.toFloat()
                packed[1] = size.toFloat()
                GLES20.glClearColor(0f, 0f, 0f, 0f)
                GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
                orb.draw(packed, size, size)
                val pixels = ByteBuffer.allocateDirect(size * size * 4).order(ByteOrder.nativeOrder())
                GLES20.glReadPixels(0, 0, size, size, GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, pixels)
                val golden = inflate(asset("orb-golden/" + frame.getString("file")), size * size * 4)
                var sum = 0L
                for (y in 0 until size) for (x in 0 until size * 4) {
                    // GL rows start at the bottom; goldens start at the top.
                    val a = pixels.get((size - 1 - y) * size * 4 + x).toInt() and 0xff
                    val b = golden[y * size * 4 + x].toInt() and 0xff
                    sum += abs(a - b)
                }
                val mean = sum.toDouble() / (size * size * 4)
                if (mean > worst) { worst = mean; worstName = frame.getString("file") }
                // Design target: mean per-channel difference <= 2/255.
                assertTrue("${frame.getString("file")} mean $mean", mean <= 2.0)
            }
            println("orb golden parity (GLES): worst $worstName mean $worst; renderer ${GLES20.glGetString(GLES20.GL_RENDERER)}")
        } finally {
            EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT)
            EGL14.eglDestroySurface(display, surface)
            EGL14.eglDestroyContext(display, context)
            EGL14.eglTerminate(display)
        }
    }

    /** Unattended runs must never open the microphone or start the service. */
    @Test
    fun silentModeNeverStartsTheMicrophoneService() {
        NotdotState.silent = true
        try {
            val effects = NotdotState.handle(SessionEvent.Start(StartSource.APP), microphonePermitted = true)
            assertTrue(!effects.microphone)
        } finally {
            NotdotState.silent = false
            NotdotState.handle(SessionEvent.Stop)
        }
    }

    /**
     * Pairs with notdot-link's desktop_sim on the host. Skipped unless the
     * runner passes `-e notdotQr <pairing code>`.
     */
    @Test
    fun pairDirectThenRelayOnlyRetryCommitsOnce() = runBlocking<Unit> {
        val qr = InstrumentationRegistry.getArguments().getString("notdotQr")
        assumeTrue("no desktop simulator code", qr != null)
        val link = PhoneLink()
        assertEquals(7, link.pair(qr!!, "Emulator phone").length)
        assertTrue(link.awaitApproval() is PairResult.Approved)
        val first = link.connect(false)
        assertNotEquals(LinkRoute.DISCONNECTED, first)
        val id = newRequestId()
        val note = link.addNote(id, "Synthetic note")
        assertEquals(LinkRoute.RELAY, link.connect(true))
        assertEquals(note, link.addNote(id, "Synthetic note"))
        assertEquals(1, link.listNotes().count { it == "Synthetic note" })
        println("emulator link: first route $first, then relay; retry committed once")

        // The real refusal path: the emulator's key evidence goes to the
        // desktop simulator, whose verifier checks it against Google's
        // roots and refuses it. Nothing is enrolled.
        assertFalse(link.stepUpStatus().enrolled)
        val attester = AndroidAttester(context)
        val refusal = try {
            link.setUpStepUp(attester)
            null
        } catch (e: MobileException) {
            e
        } finally {
            attester.finish(MobileException.Denied("test: never kept"))
        }
        android.util.Log.i("notdot.test", "emulator step-up set-up refused: ${refusal?.javaClass?.simpleName}: ${refusal?.message}")
        // Denied: the desktop refused the chain. Unavailable or Offline: the
        // emulator could not make an attested key (no lock or biometric,
        // or no provisioned attestation keys), so nothing was sent.
        assertTrue("$refusal", refusal is MobileException.Denied || refusal is MobileException.Unavailable ||
            refusal is MobileException.Offline)
        assertFalse(link.stepUpStatus().enrolled)
        attester.created?.let { alias ->
            val keystore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            assertFalse("a refused key is deleted", keystore.containsAlias(alias))
        }
    }

    /** 16 kHz mono 16-bit PCM WAV from the test assets, data chunk only. */
    private fun pcm16(path: String): List<Float> {
        val bytes = asset(path)
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
        error("no data chunk in $path")
    }

    /**
     * The full desktop detector through the app's packaged models and native
     * libraries: the corpus fed in 20 ms packets, paced in real time while a
     * backup decode runs (as live). Offline audio; no microphone.
     */
    @Test
    fun fullDetectorWakesOnTheCorpusThroughThePackagedModels() {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        val engine = checkNotNull(WakeModels.engine(target)) { "wake engine" }
        val loading = System.currentTimeMillis()
        while (engine.backupState() == BackupState.LOADING && System.currentTimeMillis() - loading < 60_000) {
            engine.accept(emptyList(), 16_000u)
            Thread.sleep(20)
        }
        assertEquals(engine.backupError(), BackupState.READY, engine.backupState())
        println("emulator backup ready after ${System.currentTimeMillis() - loading} ms")
        fun detects(clip: String, gain: Float): Boolean {
            val audio = pcm16("fixtures/negative.wav").map { it * 0.3f } + List(3_200) { 0f } +
                pcm16("fixtures/$clip").map { it * gain } + List(12_800) { 0f }
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
        for (gain in listOf(0.35f, 0.05f)) for (clip in listOf("hey-ava.wav", "hey-ava-uk.wav", "hey-ava-us.wav")) {
            val woke = detects(clip, gain)
            assertTrue("$clip@$gain backup=${engine.backupState()} stats=${engine.backupStats()}", woke)
        }
        for (clip in listOf("ava.wav", "negative.wav")) for (gain in listOf(0.05f, 0.35f, 0.8f)) {
            assertFalse("$clip@$gain", detects(clip, gain))
        }
        val stats = engine.backupStats()
        println("emulator backup stats: $stats")
        assertEquals("backup stats: $stats", 0uL, stats.failures)
    }

    /** Keystore-sealed identity with a disposable key alias and folder. */
    @Test
    fun identityStoreSealsSurvivesAndRefusesTampering() {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        val tag = System.nanoTime()
        val dir = File(target.cacheDir, "identity-test-$tag")
        val store = IdentityStore(dir, "notdot-test-$tag")
        try {
            assertNull(store.identity())
            val identity = store.identityCreatingIfNeeded()
            assertEquals(32, identity.size)
            assertArrayEquals(identity, store.identityCreatingIfNeeded())
            store.savePairedRecord("notdot-paired:1:e30")
            // A new instance, as after a restart, reads the same sealed files.
            val restarted = IdentityStore(dir, "notdot-test-$tag")
            assertArrayEquals(identity, restarted.identity())
            assertEquals("notdot-paired:1:e30", restarted.pairedRecord())
            PhoneLink.restore(identity, null)
            // Nothing on disk is the plain identity.
            assertFalse(File(dir, "identity.sealed").readBytes().toList().windowed(32).any { it == identity.toList() })
            // A swapped file fails its authentication tag.
            File(dir, "identity.sealed").copyTo(File(dir, "paired.sealed"), overwrite = true)
            assertTrue(runCatching { restarted.pairedRecord() }.isFailure)
            // So does a flipped byte.
            val file = File(dir, "identity.sealed")
            val bytes = file.readBytes()
            bytes[bytes.size - 1] = (bytes[bytes.size - 1].toInt() xor 1).toByte()
            file.writeBytes(bytes)
            assertTrue(runCatching { restarted.identity() }.isFailure)
            // Unreadable storage never blocks pairing: the user's next
            // pairing starts afresh with a new identity and no record.
            val fresh = restarted.identityCreatingIfNeeded()
            assertEquals(32, fresh.size)
            assertFalse(fresh.contentEquals(identity))
            assertArrayEquals(fresh, IdentityStore(dir, "notdot-test-$tag").identity())
            assertNull(restarted.pairedRecord())
        } finally {
            store.clear()
            dir.deleteRecursively()
        }
        assertNull(IdentityStore(dir, "notdot-test-$tag").identity())
    }

    private val attestationOid = "1.3.6.1.4.1.11129.2.1.17"

    /** The attester on this device or emulator: the key is created with
     *  the computer's challenge, and its chain carries Google's key
     *  attestation extension with that challenge. Whether the chain is
     *  trusted is the computer's decision (see the desktop verifier); here
     *  only its shape is checked. Without a screen lock and biometric the
     *  key can't be created, which is reported as unavailable. */
    @Test
    fun attesterBindsTheChallengeIntoTheKeysChain() {
        val challenge = ByteArray(32) { (it * 7 + 1).toByte() }
        val attester = AndroidAttester(context)
        try {
            val evidence = attester.attest(AttestRequest(challenge, ByteArray(32), ByteArray(32), ByteArray(32)))
            val chain = (evidence as StepUpEvidence.AndroidKeystore).chain
            assertTrue(chain.size >= 2)
            val factory = java.security.cert.CertificateFactory.getInstance("X.509")
            val leaf = factory.generateCertificate(chain[0].inputStream()) as java.security.cert.X509Certificate
            val extension = leaf.getExtensionValue(attestationOid)
            assertTrue("leaf carries key attestation", extension != null)
            assertTrue("the challenge is attested", extension!!.toList().windowed(32).any { it == challenge.toList() })
            android.util.Log.i("notdot.test", "attested chain: ${chain.size} certificates, root ${
                (factory.generateCertificate(chain.last().inputStream()) as java.security.cert.X509Certificate).subjectX500Principal}")
        } catch (e: MobileException.Unavailable) {
            android.util.Log.i("notdot.test", "step-up unavailable here: ${e.detail}")
        } finally {
            attester.finish(MobileException.Denied("test: never kept"))
        }
        attester.created?.let { alias ->
            val keystore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            assertFalse("a key the computer didn't accept is deleted", keystore.containsAlias(alias))
        }
    }

    /** Keystore signature format with a test key that needs no biometric
     *  (never the app's key): DER ECDSA that verifies with the SEC1 key. */
    @Test
    fun keystoreSignaturesUseTheCoresFormats() {
        val alias = "notdot-test-sign-${System.nanoTime()}"
        val keystore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        try {
            java.security.KeyPairGenerator.getInstance(android.security.keystore.KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
                initialize(
                    android.security.keystore.KeyGenParameterSpec.Builder(alias, android.security.keystore.KeyProperties.PURPOSE_SIGN)
                        .setAlgorithmParameterSpec(java.security.spec.ECGenParameterSpec("secp256r1"))
                        .setDigests(android.security.keystore.KeyProperties.DIGEST_SHA256)
                        .build(),
                )
                generateKeyPair()
            }
            val entry = keystore.getEntry(alias, null) as java.security.KeyStore.PrivateKeyEntry
            val public = entry.certificate.publicKey as java.security.interfaces.ECPublicKey
            assertEquals(65, StepUpKeys.sec1(public).size)
            val message = "synthetic".toByteArray()
            val signature = java.security.Signature.getInstance("SHA256withECDSA").run {
                initSign(entry.privateKey); update(message); sign()
            }
            assertEquals(0x30.toByte(), signature[0])
            assertTrue(java.security.Signature.getInstance("SHA256withECDSA").run {
                initVerify(public); update(message); verify(signature)
            })
        } finally {
            keystore.deleteEntry(alias)
        }
    }

    /** A key whose set-up reply was lost is kept only if the computer
     *  turns out to hold it. Needs a screen lock, a fingerprint and
     *  attestation keys; skipped where the key can't be made. */
    @Test
    fun aPendingKeyIsKeptOnlyIfTheComputerHoldsIt() {
        val keystore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        for (computerHasIt in listOf(true, false)) {
            val attester = AndroidAttester(context)
            try {
                attester.attest(AttestRequest(ByteArray(32), ByteArray(32), ByteArray(32), ByteArray(32)))
            } catch (e: MobileException) {
                assumeTrue("no attested key here: ${e.message}", false)
            }
            val alias = attester.created!!
            attester.finish(MobileException.Offline("reply lost"))
            assertTrue("parked, not deleted", keystore.containsAlias(alias))
            val public = StepUpKeys.sec1(keystore.getCertificate(alias).publicKey as java.security.interfaces.ECPublicKey)
            val held = if (computerHasIt) uniffi.notdot_mobile_core.stepUpKeySha256(public) else ByteArray(32)
            StepUpKeys.reconcile(context, held)
            assertEquals(computerHasIt, keystore.containsAlias(alias))
            assertEquals(if (computerHasIt) alias else null, StepUpKeys.activeAlias(context).takeIf { it == alias })
        }
        StepUpKeys.clear(context)
        assertNull(StepUpKeys.activeAlias(context))
    }

}
