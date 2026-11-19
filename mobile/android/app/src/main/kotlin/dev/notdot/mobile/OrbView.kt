package dev.notdot.mobile

import android.content.Context
import android.opengl.GLES20
import android.opengl.GLSurfaceView
import android.os.SystemClock
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.viewinterop.AndroidView
import uniffi.notdot_mobile_core.OrbEngine
import uniffi.notdot_mobile_core.OrbFrame
import uniffi.notdot_mobile_core.OrbState
import uniffi.notdot_mobile_core.OverlayKind
import uniffi.notdot_mobile_core.orbRadiusFraction
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/** The real orb: canonical shader on GLES, overlays from the contract. */
@Composable
fun Orb(state: OrbState, modifier: Modifier = Modifier) {
    var frame by remember { mutableStateOf<OrbFrame?>(null) }
    Box(
        modifier.semantics {
            contentDescription = "notdot. " + (frame?.accessibilityLabel ?: "Ready.")
            liveRegion = LiveRegionMode.Polite
        },
    ) {
        AndroidView(
            factory = { context -> OrbSurface(context) { frame = it } },
            update = { it.state = state },
            modifier = Modifier.fillMaxSize(),
        )
        Canvas(Modifier.fillMaxSize()) {
            val current = frame ?: return@Canvas
            val center = Offset(size.width / 2, size.height / 2)
            val radius = minOf(size.width, size.height) * orbRadiusFraction().toFloat()
            for (overlay in current.overlays) {
                val colour = Color(overlay.rgba[0], overlay.rgba[1], overlay.rgba[2], overlay.rgba[3])
                val px = density
                when (overlay.kind) {
                    OverlayKind.GLOW -> {
                        val r = radius * overlay.radius.toFloat()
                        val stops = overlay.stops.map { it.offset.toFloat() to Color(it.rgba[0], it.rgba[1], it.rgba[2], it.rgba[3]) }
                        drawCircle(Brush.radialGradient(*stops.toTypedArray(), center = center, radius = r), r, center)
                    }
                    OverlayKind.RING -> drawCircle(colour, radius * overlay.radius.toFloat(), center, style = Stroke(overlay.widthPx.toFloat() * px))
                    OverlayKind.DOT -> drawCircle(
                        colour, overlay.sizePx.toFloat() * px,
                        Offset(center.x + radius * overlay.dx.toFloat(), center.y + radius * overlay.dy.toFloat()),
                    )
                    OverlayKind.BAR -> {
                        val y = center.y + radius * overlay.dy.toFloat()
                        val half = overlay.sizePx.toFloat() * px
                        drawLine(colour, Offset(center.x - half, y), Offset(center.x + half, y), overlay.widthPx.toFloat() * px)
                    }
                }
            }
        }
    }
}

private class OrbSurface(context: Context, onFrame: (OrbFrame) -> Unit) : GLSurfaceView(context) {
    @Volatile var state: OrbState? = null

    init {
        setEGLContextClientVersion(2)
        setEGLConfigChooser(8, 8, 8, 8, 0, 0)
        setRenderer(object : Renderer {
            private lateinit var gl: OrbGl
            private val engine = OrbEngine()
            private var packed: List<Float>? = null
            private var width = 1
            private var height = 1

            override fun onSurfaceCreated(unused: GL10?, config: EGLConfig?) {
                gl = OrbGl()
            }

            override fun onSurfaceChanged(unused: GL10?, w: Int, h: Int) {
                width = w
                height = h
            }

            override fun onDrawFrame(unused: GL10?) {
                // Match the home background (0xFF050A14); the shader's
                // premultiplied output blends over it as on desktop.
                GLES20.glClearColor(0x05 / 255f, 0x0A / 255f, 0x14 / 255f, 1f)
                GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
                val current = state ?: return
                val now = SystemClock.uptimeMillis().toDouble()
                engine.frame(current, now, width.toFloat(), height.toFloat())?.let {
                    packed = it.packed
                    post { onFrame(it) }
                }
                packed?.let { gl.draw(it, width, height) }
            }
        })
        renderMode = RENDERMODE_CONTINUOUSLY
    }
}
