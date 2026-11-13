package dev.notdot.mobile

import android.opengl.EGL14
import android.opengl.GLES20
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.notdot_mobile_core.LinkRoute
import uniffi.notdot_mobile_core.PairResult
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.newRequestId
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
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        assertEquals(
            android.content.pm.PackageManager.PERMISSION_DENIED,
            target.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO),
        )
        assertTrue(!NotdotState.effects.value.microphone)
    }

    /**
     * Pairs with notdot-link's desktop_sim on the host. Skipped unless the
     * runner passes `-e notdotQr <pairing code>`.
     */
    @Test
    fun pairDirectThenRelayOnlyRetryCommitsOnce() = runBlocking {
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
    }
}
