package dev.notdot.mobile

import android.opengl.GLES20
import uniffi.notdot_mobile_core.orbFragmentGlsl
import uniffi.notdot_mobile_core.orbQuad
import uniffi.notdot_mobile_core.orbUniformNames
import uniffi.notdot_mobile_core.orbVertexGlsl
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * The canonical orb shader on OpenGL ES 2. The fragment source is
 * crates/notdot-orb/src/orb.frag, unchanged, supplied by the Rust core.
 * This class binds `OrbFrame.packed` to the declared uniforms and draws a
 * quad; it has no colour, timing or state logic. Needs a current context.
 */
class OrbGl {
    private val program: Int
    private val position: Int
    private val locations: IntArray
    private val quad = ByteBuffer.allocateDirect(orbQuad().size * 4).order(ByteOrder.nativeOrder()).asFloatBuffer()
        .apply { put(orbQuad().toFloatArray()); position(0) }

    init {
        val vertex = compile(GLES20.GL_VERTEX_SHADER, orbVertexGlsl())
        val fragment = compile(GLES20.GL_FRAGMENT_SHADER, orbFragmentGlsl())
        program = GLES20.glCreateProgram()
        GLES20.glAttachShader(program, vertex)
        GLES20.glAttachShader(program, fragment)
        GLES20.glLinkProgram(program)
        val linked = IntArray(1)
        GLES20.glGetProgramiv(program, GLES20.GL_LINK_STATUS, linked, 0)
        check(linked[0] == GLES20.GL_TRUE) { "orb link failed: " + GLES20.glGetProgramInfoLog(program) }
        GLES20.glDeleteShader(vertex)
        GLES20.glDeleteShader(fragment)
        position = GLES20.glGetAttribLocation(program, "position")
        locations = orbUniformNames().map { GLES20.glGetUniformLocation(program, it) }.toIntArray()
    }

    private fun compile(kind: Int, source: String): Int {
        val shader = GLES20.glCreateShader(kind)
        GLES20.glShaderSource(shader, source)
        GLES20.glCompileShader(shader)
        val ok = IntArray(1)
        GLES20.glGetShaderiv(shader, GLES20.GL_COMPILE_STATUS, ok, 0)
        check(ok[0] == GLES20.GL_TRUE) { "orb shader failed: " + GLES20.glGetShaderInfoLog(shader) }
        return shader
    }

    /** Draw one frame. `packed` is `OrbFrame.packed` (14 floats). */
    fun draw(packed: List<Float>, width: Int, height: Int) {
        require(packed.size == 14) { "packed uniforms are 14 floats" }
        GLES20.glViewport(0, 0, width, height)
        GLES20.glUseProgram(program)
        // The shader outputs premultiplied colour, as the desktop canvas uses.
        GLES20.glEnable(GLES20.GL_BLEND)
        GLES20.glBlendFunc(GLES20.GL_ONE, GLES20.GL_ONE_MINUS_SRC_ALPHA)
        GLES20.glUniform2f(locations[0], packed[0], packed[1])
        GLES20.glUniform3f(locations[1], packed[2], packed[3], packed[4])
        for (index in 2 until locations.size) GLES20.glUniform1f(locations[index], packed[index + 3])
        GLES20.glEnableVertexAttribArray(position)
        GLES20.glVertexAttribPointer(position, 2, GLES20.GL_FLOAT, false, 0, quad)
        GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, 6)
        GLES20.glDisableVertexAttribArray(position)
    }
}
